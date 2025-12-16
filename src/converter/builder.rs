//! Arch Linux package builder
//!
//! Creates .pkg.tar.zst packages from extracted files and metadata

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use tar::Builder as TarBuilder;

use crate::cli::OutputFormat;
use crate::error::{RexebError, Result};
use crate::models::PackageMetadata;

use super::InstallScriptGenerator;

/// Package builder for creating Arch Linux packages
pub struct PackageConverter {
    /// Package metadata
    metadata: PackageMetadata,
    /// Path to extracted data files
    data_dir: PathBuf,
}

impl PackageConverter {
    /// Create a new package converter
    pub fn new(metadata: PackageMetadata, data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        
        if !data_dir.exists() {
            return Err(RexebError::file_not_found(&data_dir));
        }

        Ok(Self { metadata, data_dir })
    }

    /// Build the Arch Linux package
    pub fn build(&self, output_dir: &Path, format: OutputFormat) -> Result<PathBuf> {
        let package_name = format!(
            "{}-{}-{}.{}",
            self.metadata.effective_name(),
            self.metadata.version,
            self.metadata.release,
            self.metadata.arch.to_arch_name()
        );

        let output_path = output_dir.join(format!("{}.{}", package_name, format.extension()));

        // Create temporary directory for package contents
        let temp_dir = tempfile::TempDir::new()?;
        let pkg_root = temp_dir.path();

        // Create .BUILDINFO
        self.create_buildinfo(pkg_root)?;

        // Create .PKGINFO
        self.create_pkginfo(pkg_root)?;

        // Create .INSTALL if there are maintainer scripts
        self.create_install_script(pkg_root)?;

        // Copy data files
        self.copy_data_files(pkg_root)?;

        // Create .MTREE (file metadata tree) - MUST be after all files are in place
        self.create_mtree(pkg_root)?;

        // Build the tar archive with compression
        self.create_archive(&output_path, pkg_root, format)?;

        Ok(output_path)
    }

    /// Create .BUILDINFO file
    fn create_buildinfo(&self, pkg_root: &Path) -> Result<()> {
        let buildinfo_path = pkg_root.join(".BUILDINFO");
        let content = self.generate_buildinfo();
        fs::write(buildinfo_path, content)?;
        Ok(())
    }

    /// Generate .BUILDINFO content
    fn generate_buildinfo(&self) -> String {
        let mut lines = Vec::new();

        lines.push("format = 2".to_string());
        lines.push(format!("pkgname = {}", self.metadata.effective_name()));
        lines.push(format!("pkgbase = {}", self.metadata.effective_name()));
        lines.push(format!("pkgver = {}", self.metadata.full_version()));
        lines.push(format!("pkgarch = {}", self.metadata.arch.to_arch_name()));

        // Generate a simple SHA256 based on package name and version for consistency
        use sha2::{Sha256, Digest};
        let hash_input = format!("{}:{}", self.metadata.effective_name(), self.metadata.full_version());
        let hash = Sha256::new().chain_update(hash_input).finalize();
        let hash_hex = hex::encode(hash);
        lines.push(format!("pkgbuild_sha256sum = {}", &hash_hex[..32])); // Truncate to reasonable length

        lines.push(format!("packager = {} (converted by rexeb)", self.metadata.maintainer.as_deref().unwrap_or("Unknown")));
        lines.push(format!("builddate = {}", chrono::Utc::now().timestamp()));
        lines.push("builddir = /tmp/rexeb".to_string());
        lines.push("startdir = /tmp/rexeb".to_string());
        lines.push("buildtool = rexeb".to_string());
        lines.push("buildtoolver = 0.1.0".to_string());
        lines.push("buildenv = !distcc".to_string());
        lines.push("buildenv = !ccache".to_string());
        lines.push("buildenv = !check".to_string());
        lines.push("buildenv = !sign".to_string());
        lines.push("options = !strip".to_string()); // Converted packages typically preserve original stripping
        lines.push("options = !docs".to_string());
        lines.push("options = !libtool".to_string());
        lines.push("options = !staticlibs".to_string());

        lines.join("\n")
    }

    /// Create .PKGINFO file
    fn create_pkginfo(&self, pkg_root: &Path) -> Result<()> {
        let pkginfo_path = pkg_root.join(".PKGINFO");
        let content = self.metadata.to_pkginfo();
        fs::write(pkginfo_path, content)?;
        Ok(())
    }

    /// Create .MTREE file (file metadata)
    fn create_mtree(&self, pkg_root: &Path) -> Result<()> {
        let mtree_path = pkg_root.join(".MTREE");

        // Generate MTREE content - must use #mtree header
        let mut mtree_content = String::new();
        mtree_content.push_str("#mtree\n");
        mtree_content.push_str("/set type=file uid=0 gid=0 mode=644\n");

        // Add special files to MTREE (excluding .MTREE itself - can't hash itself)
        let special_files = [".BUILDINFO", ".PKGINFO", ".INSTALL"];
        for filename in special_files {
            let path = pkg_root.join(filename);
            if path.exists() {
                if let Ok(metadata) = path.metadata() {
                    let size = metadata.len();
                    // Generate a simple SHA256 for the file
                    if let Ok(content) = std::fs::read(&path) {
                        use sha2::{Sha256, Digest};
                        let hash = Sha256::new().chain_update(&content).finalize();
                        let hash_hex = hex::encode(hash);
                        mtree_content.push_str(&format!(
                            "./{} time=0 size={} sha256digest={}\n",
                            filename, size, hash_hex
                        ));
                    }
                }
            }
        }

        mtree_content.push_str("/set mode=755\n");

        // Add data files from pkg_root (all files except special ones)
        for entry in walkdir::WalkDir::new(pkg_root) {
            let entry = entry?;
            if let Ok(rel_path) = entry.path().strip_prefix(pkg_root) {
                if rel_path.as_os_str().is_empty() {
                    continue;
                }

                let path_str = rel_path.to_string_lossy();
                
                // Skip special files (already handled above)
                if path_str.starts_with(".BUILDINFO")
                    || path_str.starts_with(".PKGINFO")
                    || path_str.starts_with(".MTREE")
                    || path_str.starts_with(".INSTALL")
                {
                    continue;
                }

                let metadata = entry.metadata()?;

                let file_type = if metadata.is_dir() {
                    "dir"
                } else if metadata.is_file() {
                    "file"
                } else if metadata.file_type().is_symlink() {
                    "link"
                } else {
                    continue;
                };

                #[cfg(unix)]
                let mode = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o7777
                };
                #[cfg(not(unix))]
                let mode = if metadata.is_dir() { 755 } else { 644 };

                if metadata.is_dir() {
                    mtree_content.push_str(&format!(
                        "./{} time=0 mode={:o} type={}\n",
                        path_str, mode, file_type
                    ));
                } else if metadata.is_file() {
                    let size = metadata.len();
                    // Generate SHA256 for regular files
                    if let Ok(content) = std::fs::read(entry.path()) {
                        use sha2::{Sha256, Digest};
                        let hash = Sha256::new().chain_update(&content).finalize();
                        let hash_hex = hex::encode(hash);
                        mtree_content.push_str(&format!(
                            "./{} time=0 size={} mode={:o} type={} sha256digest={}\n",
                            path_str, size, mode, file_type, hash_hex
                        ));
                    }
                } else if metadata.file_type().is_symlink() {
                    #[cfg(unix)]
                    if let Ok(target) = std::fs::read_link(entry.path()) {
                        mtree_content.push_str(&format!(
                            "./{} time=0 mode={:o} type={} link={}\n",
                            path_str, mode, file_type, target.display()
                        ));
                    }
                }
            }
        }

        // Compress MTREE with gzip
        let file = File::create(&mtree_path)?;
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        encoder.write_all(mtree_content.as_bytes())?;
        encoder.finish()?;

        Ok(())
    }

    /// Create .INSTALL file from maintainer scripts
    fn create_install_script(&self, pkg_root: &Path) -> Result<()> {
        let generator = InstallScriptGenerator::new(&self.metadata);
        
        if let Some(content) = generator.generate()? {
            let install_path = pkg_root.join(".INSTALL");
            fs::write(install_path, content)?;
        }

        Ok(())
    }

    /// Copy data files to package root
    fn copy_data_files(&self, pkg_root: &Path) -> Result<()> {
        for entry in walkdir::WalkDir::new(&self.data_dir) {
            let entry = entry?;
            let source = entry.path();
            
            if let Ok(rel_path) = source.strip_prefix(&self.data_dir) {
                if rel_path.as_os_str().is_empty() {
                    continue;
                }

                let dest = pkg_root.join(rel_path);
                
                if entry.file_type().is_dir() {
                    fs::create_dir_all(&dest)?;
                } else if entry.file_type().is_file() {
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(source, &dest)?;
                } else if entry.file_type().is_symlink() {
                    #[cfg(unix)]
                    {
                        let target = fs::read_link(source)?;
                        if dest.exists() || dest.symlink_metadata().is_ok() {
                            fs::remove_file(&dest)?;
                        }
                        std::os::unix::fs::symlink(target, &dest)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Create the compressed tar archive
    fn create_archive(&self, output: &Path, pkg_root: &Path, format: OutputFormat) -> Result<()> {
        let file = File::create(output)?;
        let buf_writer = BufWriter::new(file);

        match format {
            OutputFormat::PkgTarZst => {
                let encoder = zstd::Encoder::new(buf_writer, 19)?;
                let mut tar = TarBuilder::new(encoder.auto_finish());
                self.add_package_files(&mut tar, pkg_root)?;
            }
            OutputFormat::PkgTarXz => {
                let encoder = xz2::write::XzEncoder::new(buf_writer, 6);
                let mut tar = TarBuilder::new(encoder);
                self.add_package_files(&mut tar, pkg_root)?;
            }
            OutputFormat::PkgTarGz => {
                let encoder = flate2::write::GzEncoder::new(buf_writer, flate2::Compression::default());
                let mut tar = TarBuilder::new(encoder);
                self.add_package_files(&mut tar, pkg_root)?;
            }
        }

        Ok(())
    }

    /// Add files to tar archive with proper root ownership
    fn add_package_files<W: Write>(&self, tar: &mut TarBuilder<W>, pkg_root: &Path) -> Result<()> {
        // Add special files first (in official Arch package order)
        let special_files = [".BUILDINFO", ".MTREE", ".PKGINFO", ".INSTALL"];

        for filename in special_files {
            let path = pkg_root.join(filename);
            if path.exists() {
                self.append_file_with_root_owner(tar, &path, Path::new(filename))?;
            }
        }

        // Add data files
        for entry in walkdir::WalkDir::new(pkg_root)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !name.starts_with(".BUILDINFO")
                    && !name.starts_with(".PKGINFO")
                    && !name.starts_with(".MTREE")
                    && !name.starts_with(".INSTALL")
            })
        {
            let entry = entry?;
            let path = entry.path();
            
            if let Ok(rel_path) = path.strip_prefix(pkg_root) {
                if rel_path.as_os_str().is_empty() {
                    continue;
                }

                if entry.file_type().is_file() {
                    self.append_file_with_root_owner(tar, path, rel_path)?;
                } else if entry.file_type().is_dir() {
                    self.append_dir_with_root_owner(tar, path, rel_path)?;
                } else if entry.file_type().is_symlink() {
                    #[cfg(unix)]
                    {
                        let target = fs::read_link(path)?;
                        self.append_symlink_with_root_owner(tar, rel_path, &target)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Append a file to tar with root ownership (uid=0, gid=0)
    fn append_file_with_root_owner<W: Write>(
        &self,
        tar: &mut TarBuilder<W>,
        path: &Path,
        name: &Path,
    ) -> Result<()> {
        let metadata = path.metadata()?;
        let mut header = tar::Header::new_gnu();
        
        header.set_size(metadata.len());
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(metadata.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());
        
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            header.set_mode(metadata.permissions().mode());
        }
        #[cfg(not(unix))]
        {
            header.set_mode(0o644);
        }
        
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        
        let file = File::open(path)?;
        tar.append_data(&mut header, name, file)?;
        
        Ok(())
    }

    /// Append a directory to tar with root ownership (uid=0, gid=0)
    fn append_dir_with_root_owner<W: Write>(
        &self,
        tar: &mut TarBuilder<W>,
        path: &Path,
        name: &Path,
    ) -> Result<()> {
        let metadata = path.metadata()?;
        let mut header = tar::Header::new_gnu();
        
        header.set_size(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(metadata.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());
        
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            header.set_mode(metadata.permissions().mode());
        }
        #[cfg(not(unix))]
        {
            header.set_mode(0o755);
        }
        
        header.set_entry_type(tar::EntryType::Directory);
        header.set_cksum();
        
        // Ensure directory name ends with slash
        let name_str = name.to_string_lossy();
        let name_with_slash = if !name_str.ends_with('/') {
            format!("{}/", name_str)
        } else {
            name_str.to_string()
        };
        
        tar.append_data(&mut header, Path::new(&name_with_slash), std::io::empty())?;
        
        Ok(())
    }

    /// Append a symlink to tar with root ownership (uid=0, gid=0)
    #[cfg(unix)]
    fn append_symlink_with_root_owner<W: Write>(
        &self,
        tar: &mut TarBuilder<W>,
        name: &Path,
        target: &Path,
    ) -> Result<()> {
        let mut header = tar::Header::new_gnu();
        
        header.set_size(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_mode(0o777);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_cksum();
        
        tar.append_link(&mut header, name, target)?;
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_package_name_generation() {
        let metadata = PackageMetadata::new("test-package", "1.0.0");
        let temp_dir = TempDir::new().unwrap();
        
        // Create a dummy data directory
        let data_dir = temp_dir.path().join("data");
        fs::create_dir_all(&data_dir).unwrap();
        
        let converter = PackageConverter::new(metadata, &data_dir).unwrap();
        
        // Verify metadata is set correctly
        assert_eq!(converter.metadata.name, "test-package");
        assert_eq!(converter.metadata.version, "1.0.0");
    }
}
