//! Arch Linux package builder
//!
//! Creates .pkg.tar.zst packages from extracted files and metadata

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tar::Builder as TarBuilder;

use crate::cli::OutputFormat;
use crate::error::{RexebError, Result};
use crate::models::PackageMetadata;
use crate::sandbox::{NspawnSandbox, Sandbox};

use super::InstallScriptGenerator;

/// Package builder for creating Arch Linux packages
pub struct PackageConverter {
    /// Package metadata
    metadata: PackageMetadata,
    /// Path to extracted data files
    data_dir: PathBuf,
    /// Optional sandbox for isolated builds
    sandbox: Option<NspawnSandbox>,
    /// Overwrite an existing output file (from `--force`)
    overwrite: bool,
    /// Strip ELF binaries after staging (from config `strip_binaries`)
    strip_binaries: bool,
    /// Original source filename (for the provenance sentinel)
    source_file: Option<String>,
}

impl PackageConverter {
    /// Create a new package converter
    pub fn new(metadata: PackageMetadata, data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();

        if !data_dir.exists() {
            return Err(RexebError::file_not_found(&data_dir));
        }

        Ok(Self {
            metadata,
            data_dir,
            sandbox: None,
            overwrite: false,
            strip_binaries: false,
            source_file: None,
        })
    }

    /// Enable sandboxed builds using systemd-nspawn
    pub fn with_sandbox(mut self, sandbox_root: &Path) -> Result<Self> {
        let mut sandbox = NspawnSandbox::new(sandbox_root)?;
        sandbox.init()?;
        self.sandbox = Some(sandbox);
        Ok(self)
    }

    /// Allow overwriting an existing output file (default: refuse)
    pub fn with_overwrite(mut self, overwrite: bool) -> Self {
        self.overwrite = overwrite;
        self
    }

    /// Strip ELF binaries after staging (default: off)
    pub fn with_strip_binaries(mut self, strip: bool) -> Self {
        self.strip_binaries = strip;
        self
    }

    /// Record the original source filename for provenance tracking
    pub fn with_source_file(mut self, name: impl Into<String>) -> Self {
        self.source_file = Some(name.into());
        self
    }

    /// File name of the package this metadata + format would produce
    ///
    /// Shared by `build()` and `--dry-run` so the previewed name always
    /// matches the real artifact.
    pub fn package_file_name(metadata: &PackageMetadata, format: OutputFormat) -> String {
        format!(
            "{}-{}-{}.{}.{}",
            metadata.effective_name(),
            metadata.version,
            metadata.release,
            metadata.arch.to_arch_name(),
            format.extension()
        )
    }

    /// Build the Arch Linux package
    pub fn build(&self, output_dir: &Path, format: OutputFormat) -> Result<PathBuf> {
        std::fs::create_dir_all(output_dir)?;

        let output_path = output_dir.join(Self::package_file_name(&self.metadata, format));
        if output_path.exists() && !self.overwrite {
            return Err(RexebError::PackageBuild(format!(
                "Output {} already exists (use --force to overwrite)",
                output_path.display()
            )));
        }

        // Create temporary directory for package contents
        let temp_dir = tempfile::Builder::new().prefix("rexeb-").tempdir()?;
        let pkg_root = temp_dir.path();

        // Create .PKGINFO
        self.create_pkginfo(pkg_root)?;

        // Create .INSTALL if there are maintainer scripts
        self.create_install_script(pkg_root)?;

        // Copy data files
        self.copy_data_files(pkg_root)?;

        // Optionally strip ELF binaries (best effort)
        if self.strip_binaries {
            self.strip_elf_binaries(pkg_root)?;
        }

        // Write the .rexeb.json provenance sentinel so `list-installed`
        // can find this package even if pacman drops unknown PKGINFO keys
        self.create_sentinel(pkg_root)?;

        // Create .BUILDINFO (after staging: pkgbuild_sha256sum covers the
        // actual staged content)
        self.create_buildinfo(pkg_root)?;

        // Create .MTREE (file metadata tree) - MUST be after all files are in place
        self.create_mtree(pkg_root)?;

        // Build the tar archive with compression
        self.create_archive(&output_path, pkg_root, format)?;

        Ok(output_path)
    }

    /// Create .BUILDINFO file
    fn create_buildinfo(&self, pkg_root: &Path) -> Result<()> {
        let buildinfo_path = pkg_root.join(".BUILDINFO");
        let content = self.generate_buildinfo(pkg_root);
        fs::write(buildinfo_path, content)?;
        Ok(())
    }

    /// Generate .BUILDINFO content
    fn generate_buildinfo(&self, pkg_root: &Path) -> String {
        let mut lines = Vec::new();

        lines.push("format = 2".to_string());
        lines.push(format!("pkgname = {}", self.metadata.effective_name()));
        lines.push(format!("pkgbase = {}", self.metadata.effective_name()));
        lines.push(format!("pkgver = {}", self.metadata.full_version()));
        lines.push(format!("pkgarch = {}", self.metadata.arch.to_arch_name()));

        // Hash of the actual staged content (sorted path/size/digest lines),
        // so identical conversions produce identical checksums
        let tree_hash = staged_tree_hash(pkg_root).unwrap_or_else(|_| "0".repeat(64));
        lines.push(format!("pkgbuild_sha256sum = {}", &tree_hash[..32.min(tree_hash.len())]));

        lines.push(format!("packager = {} (converted by rexeb)", self.metadata.maintainer.as_deref().unwrap_or("Unknown")));
        lines.push(format!("builddate = {}", crate::build_timestamp()));
        lines.push("builddir = /tmp/rexeb".to_string());
        lines.push("startdir = /tmp/rexeb".to_string());
        lines.push("buildtool = rexeb".to_string());
        lines.push(format!("buildtoolver = {}", crate::VERSION));
        lines.push("buildenv = !distcc".to_string());
        lines.push("buildenv = !ccache".to_string());
        lines.push("buildenv = !check".to_string());
        lines.push("buildenv = !sign".to_string());
        if self.strip_binaries {
            lines.push("options = strip".to_string());
        } else {
            lines.push("options = !strip".to_string()); // Converted packages typically preserve original stripping
        }
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
                    if let Ok(meta) = path.metadata() {
                        let size = meta.len();
                        if let Ok(hash_hex) = file_sha256(&path) {
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

                // Use the link itself, not its target: `metadata()` follows
                // symlinks, so a symlink-to-dir was previously recorded as a
                // directory (and symlink-to-file hashed as a file).
                let file_type = entry.file_type();
                let metadata = std::fs::symlink_metadata(entry.path())?;

                #[cfg(unix)]
                let mode = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o7777
                };
                #[cfg(not(unix))]
                let mode = if file_type.is_dir() { 755 } else { 644 };

                if file_type.is_dir() {
                    mtree_content.push_str(&format!(
                        "./{} time=0 mode={:o} type=dir\n",
                        path_str, mode
                    ));
                } else if file_type.is_symlink() {
                    if let Ok(target) = std::fs::read_link(entry.path()) {
                        mtree_content.push_str(&format!(
                            "./{} time=0 mode={:o} type=link link={}\n",
                            path_str, mode, target.display()
                        ));
                    }
                } else if file_type.is_file() {
                    let size = metadata.len();
                    if let Ok(hash_hex) = file_sha256(entry.path()) {
                        mtree_content.push_str(&format!(
                            "./{} time=0 size={} mode={:o} type=file sha256digest={}\n",
                            path_str, size, mode, hash_hex
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
        if let Some(ref sandbox) = self.sandbox {
            // Stage a copy inside the sandbox root for inspection. NOTE: this
            // is not full isolation (archive assembly still runs on the
            // host); `--sandbox` remains experimental until the build itself
            // executes inside nspawn.
            sandbox.copy_in(&self.data_dir, Path::new("/rexeb-data"))?;
            tracing::warn!(
                "--sandbox is experimental: files are staged through the sandbox, \
                 but archive assembly still runs on the host"
            );
        }
        Self::copy_tree(&self.data_dir, pkg_root)
    }

    /// Recursively copy a tree, preserving symlinks
    fn copy_tree(src_root: &Path, dst_root: &Path) -> Result<()> {
        for entry in walkdir::WalkDir::new(src_root) {
            let entry = entry?;
            let source = entry.path();

            if let Ok(rel_path) = source.strip_prefix(src_root) {
                if rel_path.as_os_str().is_empty() {
                    continue;
                }

                let dest = dst_root.join(rel_path);

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

    /// Write the `.rexeb.json` provenance sentinel into the package
    ///
    /// Installed to `usr/share/doc/<pkg>/` so `list-installed` keeps working
    /// even when pacman drops the unknown `x-rexeb` keys from `.PKGINFO`.
    fn create_sentinel(&self, pkg_root: &Path) -> Result<()> {
        let source = self.source_file.clone().unwrap_or_else(|| {
            self.metadata.name.clone()
        });
        let watermark = crate::watermark::Watermark::new(
            &source,
            self.metadata.effective_name(),
            &self.metadata.full_version(),
            self.metadata.arch.to_arch_name(),
        );
        match watermark.to_json() {
            Ok(json) => {
                let dir = pkg_root
                    .join("usr/share/doc")
                    .join(self.metadata.effective_name());
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join(".rexeb.json"), json)?;
            }
            Err(e) => {
                tracing::warn!("Could not serialize watermark sentinel: {}", e);
            }
        }
        Ok(())
    }

    /// Strip ELF binaries in the staged tree (best effort)
    ///
    /// Only files with an ELF magic header are touched; a missing `strip`
    /// binary (or a per-file failure) is logged, never fatal.
    fn strip_elf_binaries(&self, pkg_root: &Path) -> Result<()> {
        for entry in walkdir::WalkDir::new(pkg_root) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !is_elf(path) {
                continue;
            }
            match std::process::Command::new("strip")
                .arg("--strip-unneeded")
                .arg(path)
                .output()
            {
                Ok(output) if output.status.success() => {}
                Ok(output) => {
                    tracing::debug!(
                        "strip failed for {}: {}",
                        path.display(),
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                }
                Err(e) => {
                    tracing::warn!("`strip` not available ({}); skipping binary stripping", e);
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Create the compressed tar archive
    ///
    /// Encoders are explicitly finished and the writer flushed so I/O errors
    /// (e.g. disk full) surface here instead of being swallowed by `Drop`.
    fn create_archive(&self, output: &Path, pkg_root: &Path, format: OutputFormat) -> Result<()> {
        let file = File::create(output)?;
        let buf_writer = BufWriter::new(file);

        match format {
            OutputFormat::PkgTarZst => {
                let encoder = zstd::Encoder::new(buf_writer, 19)?;
                let mut tar = TarBuilder::new(encoder);
                self.add_package_files(&mut tar, pkg_root)?;
                let encoder = tar.into_inner()?;
                let mut writer = encoder.finish()?;
                writer.flush()?;
            }
            OutputFormat::PkgTarXz => {
                let encoder = xz2::write::XzEncoder::new(buf_writer, 6);
                let mut tar = TarBuilder::new(encoder);
                self.add_package_files(&mut tar, pkg_root)?;
                let encoder = tar.into_inner()?;
                let mut writer = encoder.finish()?;
                writer.flush()?;
            }
            OutputFormat::PkgTarGz => {
                let encoder = flate2::write::GzEncoder::new(buf_writer, flate2::Compression::default());
                let mut tar = TarBuilder::new(encoder);
                self.add_package_files(&mut tar, pkg_root)?;
                let encoder = tar.into_inner()?;
                let mut writer = encoder.finish()?;
                writer.flush()?;
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
            // Mask off file-type bits: Permissions::mode() returns the full
            // st_mode, but tar wants permission bits only.
            header.set_mode(metadata.permissions().mode() & 0o7777);
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
            header.set_mode(metadata.permissions().mode() & 0o7777);
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

/// Check for an ELF magic header (used to select stripping candidates)
fn is_elf(path: &Path) -> bool {
    if let Ok(mut file) = File::open(path) {
        let mut magic = [0u8; 4];
        if file.read_exact(&mut magic).is_ok() {
            return magic == [0x7f, b'E', b'L', b'F'];
        }
    }
    false
}

/// Hash the staged tree: SHA256 over sorted `path\0size\0digest\0` records
///
/// `.BUILDINFO` itself is excluded (it does not exist yet when this runs),
/// everything else — including `.PKGINFO` and the sentinel — is covered.
fn staged_tree_hash(pkg_root: &Path) -> Result<String> {
    let mut records: Vec<String> = Vec::new();
    for entry in walkdir::WalkDir::new(pkg_root) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(pkg_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if rel == ".BUILDINFO" {
            continue;
        }
        let size = entry.metadata()?.len();
        let digest = file_sha256(entry.path())?;
        records.push(format!("{}\0{}\0{}\0", rel, size, digest));
    }
    records.sort();
    let mut hasher = Sha256::new();
    for record in &records {
        hasher.update(record.as_bytes());
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Compute SHA256 digest of a file using streaming reads (memory-efficient)
fn file_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
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
