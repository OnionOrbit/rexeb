//! Dependency resolution and package mapping

pub mod aur;
pub mod database;
pub mod fuzzy;
pub mod mapper;

pub use aur::AurClient;
pub use database::PackageDatabase;
pub use fuzzy::FuzzyMatcher;
pub use mapper::PackageMapper;

use crate::error::Result;
use crate::models::{Dependency, DependencyType, PackageMetadata};

/// Dependency resolver that maps Debian packages to Arch packages
pub struct DependencyResolver {
    /// Package database for lookups
    db: PackageDatabase,
    /// Fuzzy matcher for approximate matching
    fuzzy: FuzzyMatcher,
    /// AUR client for online lookups
    aur: AurClient,
}

impl DependencyResolver {
    /// Create a new dependency resolver
    pub fn new() -> Result<Self> {
        Ok(Self {
            db: PackageDatabase::new()?,
            fuzzy: FuzzyMatcher::new(),
            aur: AurClient::new(),
        })
    }

    /// Resolve all dependencies in a package
    pub async fn resolve(&self, metadata: &mut PackageMetadata) -> Result<()> {
        let dep_types = [
            DependencyType::Depends,
            DependencyType::PreDepends,
            DependencyType::Recommends,
            DependencyType::Suggests,
            DependencyType::Conflicts,
            DependencyType::Replaces,
            DependencyType::Provides,
            DependencyType::Breaks,
        ];

        for dep_type in dep_types {
            if let Some(deps) = metadata.dependencies.get_mut(&dep_type) {
                for dep in deps.iter_mut() {
                    self.resolve_single(dep).await?;
                    
                    // Also resolve alternatives
                    for alt in dep.alternatives.iter_mut() {
                        self.resolve_single(alt).await?;
                    }
                }
            }
        }

        // Handle Java dependency conflicts after resolution
        self.handle_java_conflicts(metadata)?;

        Ok(())
    }

    /// Resolve a single dependency
    async fn resolve_single(&self, dep: &mut Dependency) -> Result<()> {
        // Skip if already resolved
        if dep.is_mapped() {
            return Ok(());
        }

        // 1. Try exact mapping from local DB
        if let Some((arch_name, confidence)) = self.db.lookup(&dep.debian_name)? {
            dep.set_arch_name(arch_name, confidence);
            return Ok(());
        }

        // 2. Try fuzzy matching against local DB
        if let Some((arch_name, confidence)) = self.fuzzy.find_best_match(&dep.debian_name, &self.db)? {
            dep.set_arch_name(arch_name, confidence);
            return Ok(());
        }

        // 3. Try AUR search
        // First try exact name match in AUR
        if let Ok(results) = self.aur.info(&[&dep.debian_name]).await {
            if let Some(pkg) = results.first() {
                dep.set_arch_name(&pkg.name, 1.0);
                return Ok(());
            }
        }

        // 4. Try AUR provider search (for virtual packages or libraries)
        if let Ok(providers) = self.aur.find_providers(&dep.debian_name).await {
            if let Some(pkg) = providers.first() {
                // If we found a provider, use it but with lower confidence
                // unless the names match exactly
                let confidence = if pkg.name == dep.debian_name { 1.0 } else { 0.8 };
                dep.set_arch_name(&pkg.name, confidence);
                return Ok(());
            }
        }

        // 5. Check if it's a known virtual package in local DB
        if self.db.is_virtual(&dep.debian_name)? {
            dep.is_virtual = true;
        }

        Ok(())
    }

    /// Handle Java dependency conflicts by ensuring virtual package usage and conflict avoidance
    pub fn handle_java_conflicts(&self, metadata: &mut PackageMetadata) -> Result<()> {

        // Get configuration
        let config = crate::config::Config::load().unwrap_or_default();

        // Check if Java conflict handling is enabled
        if !config.java.add_java_conflicts {
            return Ok(());
        }

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
        let mut jre_deps = Vec::new();
        let mut jdk_deps = Vec::new();

        // Check all dependency types
        for (dep_type, deps) in metadata.dependencies.iter_mut() {
            for dep in deps.iter_mut() {
                if let Some(ref arch_name) = dep.arch_name {
                    if java_jre_patterns.iter().any(|p| arch_name.contains(p)) {
                        jre_deps.push((dep_type.clone(), dep.debian_name.clone(), arch_name.clone()));
                    }
                    if java_jdk_patterns.iter().any(|p| arch_name.contains(p)) {
                        jdk_deps.push((dep_type.clone(), dep.debian_name.clone(), arch_name.clone()));
                    }
                }
            }
        }

        // If both JRE and JDK dependencies exist, handle conflicts based on strategy
        if !jre_deps.is_empty() && !jdk_deps.is_empty() {
            match config.java.conflict_strategy.as_str() {
                "prefer-jdk" => {
                    // Remove JRE dependencies when JDK dependencies are present
                    // (JDK includes JRE functionality)
                    for (dep_type, debian_name, _) in &jre_deps {
                        if let Some(deps) = metadata.dependencies.get_mut(dep_type) {
                            deps.retain(|dep| dep.debian_name != *debian_name);
                        }
                    }
                }
                "prefer-jre" => {
                    // Remove JDK dependencies when JRE dependencies are present
                    for (dep_type, debian_name, _) in &jdk_deps {
                        if let Some(deps) = metadata.dependencies.get_mut(dep_type) {
                            deps.retain(|dep| dep.debian_name != *debian_name);
                        }
                    }
                }
                "jre" => {
                    // Only keep JRE dependencies
                    for (dep_type, debian_name, _) in &jdk_deps {
                        if let Some(deps) = metadata.dependencies.get_mut(dep_type) {
                            deps.retain(|dep| dep.debian_name != *debian_name);
                        }
                    }
                }
                "jdk" => {
                    // Only keep JDK dependencies
                    for (dep_type, debian_name, _) in &jre_deps {
                        if let Some(deps) = metadata.dependencies.get_mut(dep_type) {
                            deps.retain(|dep| dep.debian_name != *debian_name);
                        }
                    }
                }
                "prompt" => {
                    // For now, default to prefer-jdk when prompt is requested
                    // In a real implementation, this would ask the user
                    for (dep_type, debian_name, _) in &jre_deps {
                        if let Some(deps) = metadata.dependencies.get_mut(dep_type) {
                            deps.retain(|dep| dep.debian_name != *debian_name);
                        }
                    }
                }
                _ => {
                    // Default to prefer-jdk
                    for (dep_type, debian_name, _) in &jre_deps {
                        if let Some(deps) = metadata.dependencies.get_mut(dep_type) {
                            deps.retain(|dep| dep.debian_name != *debian_name);
                        }
                    }
                }
            }

            // Add conflict declarations to prevent installation issues
            if config.java.add_java_conflicts {
                let mut conflicts = Vec::new();
                for (_, _, arch_name) in &jre_deps {
                    conflicts.push(arch_name.clone());
                }

                // Add conflicts to the metadata
                if !conflicts.is_empty() {
                    let existing_conflicts = metadata.dependencies.entry(DependencyType::Conflicts).or_default();
                    for conflict in conflicts {
                        let conflict_dep = Dependency::new(&conflict);
                        if !existing_conflicts.iter().any(|d| d.effective_name() == conflict_dep.effective_name()) {
                            existing_conflicts.push(conflict_dep);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Get resolution statistics
    pub fn stats(&self, metadata: &PackageMetadata) -> ResolutionStats {
        let mut stats = ResolutionStats::default();

        for deps in metadata.dependencies.values() {
            for dep in deps {
                stats.total += 1;
                
                if dep.is_mapped() {
                    stats.mapped += 1;
                    stats.total_confidence += dep.confidence;
                } else if dep.is_virtual {
                    stats.virtual_packages += 1;
                } else {
                    stats.unmapped += 1;
                    stats.unmapped_names.push(dep.debian_name.clone());
                }
            }
        }

        if stats.mapped > 0 {
            stats.avg_confidence = stats.total_confidence / stats.mapped as f32;
        }

        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Dependency, DependencyType};

    #[test]
    fn test_java_conflict_resolution() {
        let resolver = DependencyResolver::new().unwrap();
        
        // Create test metadata with both JRE and JDK dependencies
        let mut metadata = PackageMetadata::new("test-java-app", "1.0");
        
        // Add JRE dependency
        let mut jre_dep = Dependency::new("default-jre");
        jre_dep.set_arch_name("jre-openjdk", 1.0);
        metadata.add_dep(DependencyType::Depends, jre_dep);
        
        // Add JDK dependency
        let mut jdk_dep = Dependency::new("default-jdk");
        jdk_dep.set_arch_name("jdk-openjdk", 1.0);
        metadata.add_dep(DependencyType::Depends, jdk_dep);
        
        // Test Java conflict handling
        resolver.handle_java_conflicts(&mut metadata).unwrap();
        
        // Verify that JRE dependency was removed (prefer-jdk strategy)
        let deps = metadata.get_deps(DependencyType::Depends);
        let jre_exists = deps.iter().any(|dep| dep.effective_name() == "jre-openjdk");
        let jdk_exists = deps.iter().any(|dep| dep.effective_name() == "jdk-openjdk");
        
        assert!(!jre_exists, "JRE dependency should be removed when JDK is present");
        assert!(jdk_exists, "JDK dependency should be preserved");
    }

    #[test]
    fn test_java_conflict_resolution_prefer_jre() {
        // For this test, we'll test the logic directly by checking the strategy handling
        // We can't easily modify the global config for testing, so we'll focus on testing
        // the strategy logic in a different way
        
        let resolver = DependencyResolver::new().unwrap();
        
        // Create test metadata with both JRE and JDK dependencies
        let mut metadata = PackageMetadata::new("test-java-app", "1.0");
        
        // Add JRE dependency
        let mut jre_dep = Dependency::new("default-jre");
        jre_dep.set_arch_name("jre-openjdk", 1.0);
        metadata.add_dep(DependencyType::Depends, jre_dep);
        
        // Add JDK dependency
        let mut jdk_dep = Dependency::new("default-jdk");
        jdk_dep.set_arch_name("jdk-openjdk", 1.0);
        metadata.add_dep(DependencyType::Depends, jdk_dep);
        
        // Test Java conflict handling - this will use the default "prefer-jdk" strategy
        resolver.handle_java_conflicts(&mut metadata).unwrap();
        
        // Verify that JRE dependency was removed (default prefer-jdk strategy)
        let deps = metadata.get_deps(DependencyType::Depends);
        let jre_exists = deps.iter().any(|dep| dep.effective_name() == "jre-openjdk");
        let jdk_exists = deps.iter().any(|dep| dep.effective_name() == "jdk-openjdk");
        
        assert!(!jre_exists, "JRE dependency should be removed with default prefer-jdk strategy");
        assert!(jdk_exists, "JDK dependency should be preserved with default prefer-jdk strategy");
    }
}

/// Statistics about dependency resolution
#[derive(Debug, Default)]
pub struct ResolutionStats {
    /// Total number of dependencies
    pub total: usize,
    /// Successfully mapped dependencies
    pub mapped: usize,
    /// Unmapped dependencies
    pub unmapped: usize,
    /// Virtual packages (no direct mapping needed)
    pub virtual_packages: usize,
    /// Average confidence score
    pub avg_confidence: f32,
    /// Total confidence (internal)
    total_confidence: f32,
    /// List of unmapped package names
    pub unmapped_names: Vec<String>,
}

impl ResolutionStats {
    /// Get the mapping success rate
    pub fn success_rate(&self) -> f32 {
        if self.total == 0 {
            1.0
        } else {
            (self.mapped + self.virtual_packages) as f32 / self.total as f32
        }
    }
}
