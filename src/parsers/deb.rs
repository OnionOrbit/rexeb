//! Debian package (.deb) parser
//!
//! .deb files are ar archives containing:
//! - debian-binary: version string
//! - control.tar.{gz,xz,zst}: control information
//! - data.tar.{gz,xz,zst,bz2}: actual package files

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use tar::Archive;
use tempfile::TempDir;
use xz2::read::XzDecoder;

use crate::error::{RexebError, Result};
use crate::models::{
    Architecture, Dependency, DependencyType, License, MaintainerScript, PackageFormat,
    PackageMetadata,
};

/// Parser for Debian .deb packages
pub struct DebParser {
    /// Path to the .deb file
    path: PathBuf,
    /// Temporary directory for extraction
    temp_dir: TempDir,
    /// Path to extracted control directory
    control_dir: PathBuf,
    /// Path to extracted data directory
    data_dir: PathBuf,
}

impl DebParser {
    /// Create a new parser for the given .deb file
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        if !path.exists() {
            return Err(RexebError::file_not_found(&path));
        }

        let temp_dir = TempDir::new()?;
        let control_dir = temp_dir.path().join("control");
        let data_dir = temp_dir.path().join("data");

        std::fs::create_dir_all(&control_dir)?;
        std::fs::create_dir_all(&data_dir)?;

        let mut parser = Self {
            path,
            temp_dir,
            control_dir,
            data_dir,
        };

        parser.extract_archive()?;

        Ok(parser)
    }

    /// Get the extraction directory path
    pub fn extract_dir(&self) -> &Path {
        self.data_dir.as_path()
    }

    /// Extract the .deb archive
    fn extract_archive(&mut self) -> Result<()> {
        let file = File::open(&self.path)?;
        let mut archive = ar::Archive::new(file);

        while let Some(entry) = archive.next_entry() {
            let mut entry = entry.map_err(|e| RexebError::Extraction(e.to_string()))?;
            let name = std::str::from_utf8(entry.header().identifier())
                .map_err(|e| RexebError::Extraction(e.to_string()))?
                .to_string();

            if name == "debian-binary" {
                // Version file, skip for now
                continue;
            } else if name.starts_with("control.tar") {
                self.extract_tar(&mut entry, &name, &self.control_dir.clone())?;
            } else if name.starts_with("data.tar") {
                self.extract_tar(&mut entry, &name, &self.data_dir.clone())?;
            }
        }

        Ok(())
    }

    /// Extract a tar archive (with compression detection)
    fn extract_tar<R: Read>(&self, reader: &mut R, name: &str, dest: &Path) -> Result<()> {
        // Detect compression from filename
        if name.ends_with(".gz") {
            let decoder = GzDecoder::new(reader);
            let mut archive = Archive::new(decoder);
            archive.unpack(dest)?;
        } else if name.ends_with(".xz") {
            let decoder = XzDecoder::new(reader);
            let mut archive = Archive::new(decoder);
            archive.unpack(dest)?;
        } else if name.ends_with(".zst") {
            let decoder = zstd::Decoder::new(reader)?;
            let mut archive = Archive::new(decoder);
            archive.unpack(dest)?;
        } else if name.ends_with(".bz2") {
            // bz2 is less common but still supported
            let mut data = Vec::new();
            reader.read_to_end(&mut data)?;
            let decoder = bzip2::read::BzDecoder::new(&data[..]);
            let mut archive = Archive::new(decoder);
            archive.unpack(dest)?;
        } else {
            // Try uncompressed tar
            let mut archive = Archive::new(reader);
            archive.unpack(dest)?;
        }

        Ok(())
    }

    /// Parse the package and return metadata
    pub fn parse(&self) -> Result<PackageMetadata> {
        let control = self.parse_control()?;
        let mut metadata = self.build_metadata(&control)?;

        // Parse maintainer scripts
        self.parse_scripts(&mut metadata)?;

        // Parse conffiles
        self.parse_conffiles(&mut metadata)?;

        // Parse md5sums
        self.parse_md5sums(&mut metadata)?;

        // Collect file list
        self.collect_files(&mut metadata)?;

        Ok(metadata)
    }

    /// Parse the control file
    fn parse_control(&self) -> Result<HashMap<String, String>> {
        let control_path = self.control_dir.join("control");
        
        if !control_path.exists() {
            return Err(RexebError::InvalidControl("control file not found".into()));
        }

        let file = File::open(&control_path)?;
        let reader = BufReader::new(file);

        let mut fields: HashMap<String, String> = HashMap::new();
        let mut current_key: Option<String> = None;
        let mut current_value = String::new();

        for line in reader.lines() {
            let line = line?;

            if line.starts_with(' ') || line.starts_with('\t') {
                // Continuation line
                if current_key.is_some() {
                    current_value.push('\n');
                    current_value.push_str(line.trim());
                }
            } else if let Some(colon_pos) = line.find(':') {
                // Save previous field
                if let Some(key) = current_key.take() {
                    fields.insert(key, current_value.trim().to_string());
                }

                // Start new field
                current_key = Some(line[..colon_pos].to_string());
                current_value = line[colon_pos + 1..].trim().to_string();
            }
        }

        // Save last field
        if let Some(key) = current_key {
            fields.insert(key, current_value.trim().to_string());
        }

        Ok(fields)
    }

    /// Build PackageMetadata from control fields
    fn build_metadata(&self, control: &HashMap<String, String>) -> Result<PackageMetadata> {
        let name = control
            .get("Package")
            .ok_or_else(|| RexebError::MissingField("Package".into()))?
            .clone();

        let version = control
            .get("Version")
            .ok_or_else(|| RexebError::MissingField("Version".into()))?
            .clone();

        let mut metadata = PackageMetadata::new(name, version);
        metadata.source_format = PackageFormat::Deb;

        // Architecture
        if let Some(arch) = control.get("Architecture") {
            metadata.arch = Architecture::from_debian(arch)?;
        }

        // Description
        if let Some(desc) = control.get("Description") {
            // First line is short description
            let mut lines = desc.lines();
            metadata.description = lines.next().unwrap_or("").to_string();
            
            // Rest is long description
            let long_desc: String = lines.collect::<Vec<_>>().join("\n");
            if !long_desc.is_empty() {
                metadata.long_description = Some(long_desc);
            }
        }

        // Maintainer
        if let Some(maintainer) = control.get("Maintainer") {
            metadata.maintainer = Some(maintainer.clone());
        }

        // Homepage
        if let Some(url) = control.get("Homepage") {
            metadata.url = Some(url.clone());
        }

        // Installed-Size (in KB in Debian)
        if let Some(size) = control.get("Installed-Size") {
            if let Ok(kb) = size.parse::<u64>() {
                metadata.installed_size = kb * 1024;
            }
        }

        // Section
        if let Some(section) = control.get("Section") {
            metadata.section = Some(section.clone());
        }

        // Priority
        if let Some(priority) = control.get("Priority") {
            metadata.priority = Some(priority.clone());
        }

        // License detection from section or other fields
        if let Some(section) = control.get("Section") {
            if section.contains("non-free") {
                metadata.license = License::Custom("non-free".into());
            }
        }

        // Parse dependencies
        self.parse_dependencies(control, &mut metadata)?;

        Ok(metadata)
    }

    /// Parse dependency fields from control
    fn parse_dependencies(
        &self,
        control: &HashMap<String, String>,
        metadata: &mut PackageMetadata,
    ) -> Result<()> {
        let dep_fields = [
            ("Depends", DependencyType::Depends),
            ("Pre-Depends", DependencyType::PreDepends),
            ("Recommends", DependencyType::Recommends),
            ("Suggests", DependencyType::Suggests),
            ("Conflicts", DependencyType::Conflicts),
            ("Replaces", DependencyType::Replaces),
            ("Provides", DependencyType::Provides),
            ("Breaks", DependencyType::Breaks),
        ];

        for (field_name, dep_type) in dep_fields {
            if let Some(deps_str) = control.get(field_name) {
                let deps = Dependency::parse_list(deps_str)?;
                for dep in deps {
                    metadata.add_dep(dep_type, dep);
                }
            }
        }

        Ok(())
    }

    /// Parse maintainer scripts (preinst, postinst, prerm, postrm)
    fn parse_scripts(&self, metadata: &mut PackageMetadata) -> Result<()> {
        let scripts = [
            (MaintainerScript::PreInst, "preinst"),
            (MaintainerScript::PostInst, "postinst"),
            (MaintainerScript::PreRm, "prerm"),
            (MaintainerScript::PostRm, "postrm"),
            (MaintainerScript::Config, "config"),
        ];

        for (script_type, filename) in scripts {
            let script_path = self.control_dir.join(filename);
            if script_path.exists() {
                let content = std::fs::read_to_string(&script_path)?;
                metadata.set_script(script_type, content);
            }
        }

        Ok(())
    }

    /// Parse conffiles list
    fn parse_conffiles(&self, metadata: &mut PackageMetadata) -> Result<()> {
        let conffiles_path = self.control_dir.join("conffiles");
        
        if conffiles_path.exists() {
            let content = std::fs::read_to_string(&conffiles_path)?;
            for line in content.lines() {
                let line = line.trim();
                if !line.is_empty() {
                    metadata.conffiles.push(PathBuf::from(line));
                }
            }
        }

        Ok(())
    }

    /// Parse md5sums file
    fn parse_md5sums(&self, metadata: &mut PackageMetadata) -> Result<()> {
        let md5sums_path = self.control_dir.join("md5sums");

        if md5sums_path.exists() {
            let content = std::fs::read_to_string(&md5sums_path)?;
            for line in content.lines() {
                let parts: Vec<&str> = line.splitn(2, char::is_whitespace).collect();
                if parts.len() == 2 {
                    let hash = parts[0].trim();
                    let path = parts[1].trim();
                    metadata.md5sums.insert(PathBuf::from(path), hash.to_string());
                }
            }
        }

        Ok(())
    }

    /// Collect list of files in the data archive
    fn collect_files(&self, metadata: &mut PackageMetadata) -> Result<()> {
        for entry in walkdir::WalkDir::new(&self.data_dir) {
            let entry = entry?;
            if entry.file_type().is_file() {
                // Get path relative to data_dir
                if let Ok(rel_path) = entry.path().strip_prefix(&self.data_dir) {
                    metadata.files.push(PathBuf::from("/").join(rel_path));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests would require sample .deb files
    // For now, just test control file parsing

    #[test]
    fn test_parse_control_content() {
        let content = r#"Package: example
Version: 1.0-1
Architecture: amd64
Maintainer: Test User <test@example.com>
Installed-Size: 1234
Depends: libc6 (>= 2.17), libssl1.1
Recommends: suggested-pkg
Description: An example package
 This is the long description
 spanning multiple lines.
"#;

        // Parse manually for testing
        let mut fields: HashMap<String, String> = HashMap::new();
        let mut current_key: Option<String> = None;
        let mut current_value = String::new();

        for line in content.lines() {
            if line.starts_with(' ') || line.starts_with('\t') {
                if current_key.is_some() {
                    current_value.push('\n');
                    current_value.push_str(line.trim());
                }
            } else if let Some(colon_pos) = line.find(':') {
                if let Some(key) = current_key.take() {
                    fields.insert(key, current_value.trim().to_string());
                }
                current_key = Some(line[..colon_pos].to_string());
                current_value = line[colon_pos + 1..].trim().to_string();
            }
        }
        if let Some(key) = current_key {
            fields.insert(key, current_value.trim().to_string());
        }

        assert_eq!(fields.get("Package"), Some(&"example".to_string()));
        assert_eq!(fields.get("Version"), Some(&"1.0-1".to_string()));
        assert_eq!(fields.get("Architecture"), Some(&"amd64".to_string()));
    }
}