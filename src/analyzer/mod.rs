//! Package analysis and pre-conversion checks

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::models::{DependencyType, PackageMetadata};

/// Package analyzer for pre-conversion analysis
pub struct PackageAnalyzer<'a> {
    /// Package metadata
    metadata: &'a PackageMetadata,
    /// Path to extracted data
    data_dir: &'a Path,
}

/// Analysis report
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AnalysisReport {
    /// Warning messages
    pub warnings: Vec<String>,
    /// Error messages
    pub errors: Vec<String>,
    /// Total dependency count
    pub dependency_count: usize,
    /// Mapped dependencies count
    pub mapped_count: usize,
    /// Unmapped dependency names
    pub unmapped_deps: Vec<String>,
    /// File conflicts with installed packages
    pub conflicts: Vec<String>,
    /// Number of verified files
    pub verified_files: usize,
    /// Number of files that failed verification
    pub failed_files: usize,
    /// FHS compliance issues
    pub fhs_issues: Vec<String>,
    /// Library compatibility issues
    pub lib_issues: Vec<String>,
    /// Security concerns
    pub security_issues: Vec<String>,
}

impl<'a> PackageAnalyzer<'a> {
    /// Create a new analyzer
    pub fn new(metadata: &'a PackageMetadata, data_dir: &'a Path) -> Result<Self> {
        Ok(Self { metadata, data_dir })
    }

    /// Perform full analysis
    pub fn analyze(&self, check_conflicts: bool, verify_files: bool) -> Result<AnalysisReport> {
        let mut report = AnalysisReport::default();

        // Analyze dependencies
        self.analyze_dependencies(&mut report)?;

        // Check for Java conflicts
        self.check_java_conflicts(&mut report)?;

        // Check FHS compliance
        self.check_fhs_compliance(&mut report)?;

        // Check library compatibility
        self.check_library_compatibility(&mut report)?;

        // Check for security issues
        self.check_security(&mut report)?;

        // Check file conflicts with installed packages
        if check_conflicts {
            self.check_conflicts(&mut report)?;
        }

        // Verify file integrity
        if verify_files {
            self.verify_files(&mut report)?;
        }

        // Check maintainer scripts
        self.analyze_scripts(&mut report)?;

        Ok(report)
    }

    /// Analyze dependencies
    fn analyze_dependencies(&self, report: &mut AnalysisReport) -> Result<()> {
        for dep_type in &[DependencyType::Depends, DependencyType::PreDepends] {
            for dep in self.metadata.get_deps(*dep_type) {
                report.dependency_count += 1;
                
                if dep.is_mapped() {
                    report.mapped_count += 1;
                } else if !dep.is_virtual {
                    report.unmapped_deps.push(dep.debian_name.clone());
                    report.warnings.push(format!(
                        "Unmapped dependency: {} (no Arch equivalent found)",
                        dep.debian_name
                    ));
                }

                // Check for known problematic dependencies
                if self.is_problematic_dep(&dep.debian_name) {
                    report.warnings.push(format!(
                        "Potentially problematic dependency: {}",
                        dep.debian_name
                    ));
                }
            }
        }

        Ok(())
    }

    /// Check if a dependency is known to be problematic
    fn is_problematic_dep(&self, name: &str) -> bool {
        let problematic = [
            "debconf",
            "dpkg",
            "apt",
            "update-manager",
            "ubuntu-release-upgrader",
            "snapd",
        ];
        
        problematic.iter().any(|p| name.contains(p))
    }

    /// Check FHS (Filesystem Hierarchy Standard) compliance
    fn check_fhs_compliance(&self, report: &mut AnalysisReport) -> Result<()> {
        let allowed_top_level: HashSet<&str> = [
            "bin", "boot", "dev", "etc", "home", "lib", "lib32", "lib64",
            "media", "mnt", "opt", "proc", "root", "run", "sbin", "srv",
            "sys", "tmp", "usr", "var",
        ].into_iter().collect();

        for file in &self.metadata.files {
            let path_str = file.to_string_lossy();
            
            // Get top-level directory
            let components: Vec<_> = file.components().collect();
            if components.len() > 1 {
                if let std::path::Component::Normal(name) = components[1] {
                    let name_str = name.to_string_lossy();
                    if !allowed_top_level.contains(name_str.as_ref()) {
                        report.fhs_issues.push(format!(
                            "Non-standard directory: {}",
                            path_str
                        ));
                    }
                }
            }

            // Check for Debian-specific paths
            if path_str.contains("/dpkg/") 
                || path_str.contains("/apt/")
                || path_str.contains("/debian/")
            {
                report.warnings.push(format!(
                    "Debian-specific path: {}",
                    path_str
                ));
            }
        }

        Ok(())
    }

    /// Check library compatibility
    fn check_library_compatibility(&self, report: &mut AnalysisReport) -> Result<()> {
        // Check for bundled libraries
        for file in &self.metadata.files {
            let path_str = file.to_string_lossy();
            
            if path_str.contains(".so") && !path_str.starts_with("/usr/lib") {
                // Library in non-standard location
                report.lib_issues.push(format!(
                    "Library in non-standard location: {}",
                    path_str
                ));
            }
        }

        // Check glibc version requirement (if we can detect it)
        let data_dir = self.data_dir;
        if let Ok(output) = Command::new("find")
            .arg(data_dir)
            .arg("-type")
            .arg("f")
            .arg("-executable")
            .output()
        {
            let executables = String::from_utf8_lossy(&output.stdout);
            for exec in executables.lines().take(5) {
                // Just check a few executables
                if let Ok(ldd_output) = Command::new("ldd").arg(exec).output() {
                    let ldd_str = String::from_utf8_lossy(&ldd_output.stderr);
                    if ldd_str.contains("not found") {
                        report.lib_issues.push(format!(
                            "Missing library for: {}",
                            exec
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Check for security issues
    fn check_security(&self, report: &mut AnalysisReport) -> Result<()> {
        for file in &self.metadata.files {
            let path_str = file.to_string_lossy();

            // Check for SUID/SGID binaries
            let full_path = self.data_dir.join(file.strip_prefix("/").unwrap_or(file));
            if full_path.exists() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if let Ok(meta) = full_path.metadata() {
                        let mode = meta.mode();
                        if mode & 0o4000 != 0 {
                            report.security_issues.push(format!(
                                "SUID binary: {}",
                                path_str
                            ));
                        }
                        if mode & 0o2000 != 0 {
                            report.security_issues.push(format!(
                                "SGID binary: {}",
                                path_str
                            ));
                        }
                    }
                }
            }

            // Check for world-writable files
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if let Ok(meta) = full_path.metadata() {
                    if meta.mode() & 0o002 != 0 && !meta.is_dir() {
                        report.security_issues.push(format!(
                            "World-writable file: {}",
                            path_str
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Check for Java dependency conflicts
    fn check_java_conflicts(&self, report: &mut AnalysisReport) -> Result<()> {
        // Define Java package patterns that conflict
        let java_jre_patterns = [
            "jre-openjdk",
            "jre8-openjdk",
            "jre11-openjdk",
            "jre17-openjdk",
            "jre21-openjdk",
            "jre-openjdk-headless",
            "jre8-openjdk-headless",
            "jre11-openjdk-headless",
            "jre17-openjdk-headless",
            "jre21-openjdk-headless",
        ];

        let java_jdk_patterns = [
            "jdk-openjdk",
            "jdk8-openjdk",
            "jdk11-openjdk",
            "jdk17-openjdk",
            "jdk21-openjdk",
        ];

        // Find all Java dependencies
        let mut has_jre = false;
        let mut has_jdk = false;
        let mut jre_names = Vec::new();
        let mut jdk_names = Vec::new();

        for dep_type in &[DependencyType::Depends, DependencyType::PreDepends] {
            for dep in self.metadata.get_deps(*dep_type) {
                if let Some(ref arch_name) = dep.arch_name {
                    if java_jre_patterns.iter().any(|p| arch_name.contains(p)) {
                        has_jre = true;
                        jre_names.push(arch_name.clone());
                    }
                    if java_jdk_patterns.iter().any(|p| arch_name.contains(p)) {
                        has_jdk = true;
                        jdk_names.push(arch_name.clone());
                    }
                }
            }
        }

        // Report Java conflicts
        if has_jre && has_jdk {
            report.warnings.push(format!(
                "Java dependency conflict detected: both JRE ({}) and JDK ({}) dependencies present. JDK will take precedence.",
                jre_names.join(", "),
                jdk_names.join(", ")
            ));
        }

        Ok(())
    }

    /// Check for file conflicts with installed packages
    fn check_conflicts(&self, report: &mut AnalysisReport) -> Result<()> {
        // Use pacman to check for file conflicts
        for file in &self.metadata.files {
            let path_str = file.to_string_lossy();
            
            // Skip directories
            if path_str.ends_with('/') {
                continue;
            }

            // Check if file exists on system
            if file.exists() {
                // Try to find which package owns it
                if let Ok(output) = Command::new("pacman")
                    .arg("-Qo")
                    .arg(path_str.as_ref())
                    .output()
                {
                    if output.status.success() {
                        let owner = String::from_utf8_lossy(&output.stdout);
                        report.conflicts.push(format!(
                            "{}: owned by {}",
                            path_str,
                            owner.trim()
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Verify file integrity using md5sums
    fn verify_files(&self, report: &mut AnalysisReport) -> Result<()> {
        use md5::Context;
        use std::io::Read;

        for (path, expected_md5) in &self.metadata.md5sums {
            let full_path = self.data_dir.join(path.strip_prefix("/").unwrap_or(path));
            
            if !full_path.exists() {
                report.failed_files += 1;
                report.warnings.push(format!("Missing file: {}", path.display()));
                continue;
            }

            // Calculate MD5 hash
            if let Ok(mut file) = std::fs::File::open(&full_path) {
                let mut context = Context::new();
                let mut buffer = [0u8; 8192];
                
                loop {
                    match file.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => context.consume(&buffer[..n]),
                        Err(_) => {
                            report.failed_files += 1;
                            break;
                        }
                    }
                }

                let hash = hex::encode(context.compute().0);
                if hash == *expected_md5 {
                    report.verified_files += 1;
                } else {
                    report.failed_files += 1;
                    report.warnings.push(format!(
                        "Hash mismatch for {}: expected {}, got {}",
                        path.display(),
                        expected_md5,
                        hash
                    ));
                }
            } else {
                report.failed_files += 1;
            }
        }

        Ok(())
    }

    /// Analyze maintainer scripts
    fn analyze_scripts(&self, report: &mut AnalysisReport) -> Result<()> {
        use crate::models::MaintainerScript;

        let script_types = [
            MaintainerScript::PreInst,
            MaintainerScript::PostInst,
            MaintainerScript::PreRm,
            MaintainerScript::PostRm,
        ];

        for script_type in script_types {
            if let Some(content) = self.metadata.get_script(script_type) {
                // Check for problematic commands
                let problematic_patterns = [
                    ("dpkg", "dpkg commands need translation"),
                    ("apt-get", "apt commands not available"),
                    ("update-rc.d", "init system commands need translation"),
                    ("systemctl preset", "systemd preset may behave differently"),
                    ("adduser", "adduser syntax differs on Arch"),
                ];

                for (pattern, warning) in problematic_patterns {
                    if content.contains(pattern) {
                        report.warnings.push(format!(
                            "{:?} script: {}",
                            script_type,
                            warning
                        ));
                    }
                }

                // Check for debconf usage
                if content.contains("debconf") || content.contains("db_") {
                    report.warnings.push(format!(
                        "{:?} script uses debconf which is not available on Arch",
                        script_type
                    ));
                }
            }
        }

        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_fhs_compliance() {
        let metadata = PackageMetadata::new("test", "1.0");
        let temp_dir = TempDir::new().unwrap();
        
        let analyzer = PackageAnalyzer::new(&metadata, temp_dir.path()).unwrap();
        let mut report = AnalysisReport::default();
        
        analyzer.check_fhs_compliance(&mut report).unwrap();
        // Should pass with empty files list
        assert!(report.fhs_issues.is_empty());
    }

    #[test]
    fn test_is_problematic_dep() {
        let metadata = PackageMetadata::new("test", "1.0");
        let temp_dir = TempDir::new().unwrap();
        
        let analyzer = PackageAnalyzer::new(&metadata, temp_dir.path()).unwrap();
        
        assert!(analyzer.is_problematic_dep("debconf"));
        assert!(analyzer.is_problematic_dep("dpkg-dev"));
        assert!(!analyzer.is_problematic_dep("glibc"));
    }
}
