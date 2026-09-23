//! Dependency resolution and package mapping

pub mod aur;
pub mod database;
pub mod fuzzy;
pub mod mapper;

pub use aur::AurClient;
pub use database::PackageDatabase;
pub use fuzzy::FuzzyMatcher;
pub use mapper::PackageMapper;

use std::sync::Mutex;

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
    /// Package mapper for regex-based name transformations
    mapper: Mutex<PackageMapper>,
}

/// Keep the highest-confidence resolution candidate seen so far.
fn consider_best(best: &mut Option<(String, f32)>, candidate: Option<(String, f32)>) {
    if let Some((name, confidence)) = candidate {
        let better = best.as_ref().map_or(true, |(_, b)| confidence > *b);
        if better {
            *best = Some((name, confidence));
        }
    }
}

impl DependencyResolver {
    /// Create a new dependency resolver
    pub fn new() -> Result<Self> {
        let config = crate::config::Config::load().unwrap_or_default();
        let min_score = if config.conversion.min_match_confidence.is_finite() {
            config.conversion.min_match_confidence.clamp(0.0, 1.0)
        } else {
            0.6
        };
        Ok(Self {
            db: PackageDatabase::new()?,
            fuzzy: FuzzyMatcher::new().with_min_score(min_score),
            aur: AurClient::new(),
            mapper: Mutex::new(PackageMapper::new()),
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

        // Dependencies qualified for another architecture (e.g. `[!amd64]`)
        // do not apply to this package and are dropped before resolution.
        let debian_arch = metadata.arch.to_debian_name();

        for dep_type in dep_types {
            if let Some(deps) = metadata.dependencies.get_mut(&dep_type) {
                let before = deps.len();
                deps.retain(|d| !d.is_excluded_on(debian_arch));
                let dropped = before - deps.len();
                if dropped > 0 {
                    tracing::debug!(
                        "Skipped {} {:?} dependencies excluded on {}",
                        dropped,
                        dep_type,
                        debian_arch
                    );
                }
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

        // Drop self-references (e.g. `Depends: self`, Debian upgrade guards
        // like `Breaks: self (<< 2.0)`) and promote usable alternatives —
        // otherwise pacman reports "couldn't resolve dependency" on install.
        for note in Self::prune_self_references(metadata) {
            tracing::warn!("{}", note);
        }

        Ok(())
    }

    /// Drop dependencies the package satisfies itself, rescuing alternatives
    ///
    /// A package must never depend on, conflict with, or replace itself:
    /// pacman cannot resolve such entries (`Breaks: weebots (<< 2.0)` is a
    /// common Debian upgrade guard that is meaningless on Arch). Matching is
    /// by name only (Debian name, Arch name, or anything in `Provides`;
    /// version constraints are ignored) and case-insensitive.
    ///
    /// When a dropped primary has a mapped, non-self alternative
    /// (`Depends: self | other`), the alternative is promoted instead of
    /// losing the dependency entirely. `Provides` entries are never pruned.
    ///
    /// Returns human-readable notes describing every drop/promotion.
    pub fn prune_self_references(metadata: &mut PackageMetadata) -> Vec<String> {
        use std::collections::HashSet;

        let mut notes = Vec::new();

        // Names satisfied by the package itself
        let mut own: HashSet<String> = HashSet::new();
        for name in [metadata.name.clone(), metadata.effective_name().to_string()] {
            if !name.is_empty() {
                own.insert(name.to_lowercase());
            }
        }
        for p in metadata.get_deps(DependencyType::Provides) {
            for name in [p.debian_name.clone(), p.effective_name().to_string()] {
                if !name.is_empty() {
                    own.insert(name.to_lowercase());
                }
            }
        }
        let is_self = |dep: &Dependency| {
            own.contains(&dep.debian_name.to_lowercase())
                || own.contains(&dep.effective_name().to_lowercase())
        };

        // Hard + soft dependencies: drop self-references (promoting a usable
        // alternative when one exists). Unmapped primaries with a mapped
        // alternative are rescued the same way; other unmapped primaries are
        // kept here so the caller can warn / review them interactively —
        // PKGINFO/PKGBUILD generation filters them from the output.
        for dep_type in [
            DependencyType::Depends,
            DependencyType::PreDepends,
            DependencyType::Recommends,
            DependencyType::Suggests,
        ] {
            if let Some(deps) = metadata.dependencies.get_mut(&dep_type) {
                let mut kept = Vec::with_capacity(deps.len());
                for mut dep in deps.drain(..) {
                    let dominated = is_self(&dep) || !dep.is_mapped();
                    if dominated {
                        let promoted: Option<Dependency> = dep
                            .alternatives
                            .iter()
                            .find(|a| a.is_mapped() && !is_self(a))
                            .cloned();
                        if let Some(alt) = promoted {
                            notes.push(format!(
                                "{:?} '{}': {} — using alternative '{}' ({})",
                                dep_type,
                                dep.debian_name,
                                if is_self(&dep) {
                                    "primary is a self-reference"
                                } else {
                                    "primary is unmapped"
                                },
                                alt.effective_name(),
                                alt.debian_name,
                            ));
                            dep.arch_name = alt.arch_name.clone();
                            dep.confidence = alt.confidence;
                            dep.version_op = alt.version_op;
                            dep.version = alt.version.clone();
                        } else if is_self(&dep) {
                            notes.push(format!(
                                "{:?} '{}': dropped self-reference",
                                dep_type, dep.debian_name
                            ));
                            continue;
                        }
                    }
                    kept.push(dep);
                }
                *deps = kept;
            }
        }

        // Conflicts/Breaks/Replaces against ourselves would make the pacman
        // transaction unresolvable — drop them (no alternatives here).
        for dep_type in [
            DependencyType::Conflicts,
            DependencyType::Breaks,
            DependencyType::Replaces,
        ] {
            if let Some(deps) = metadata.dependencies.get_mut(&dep_type) {
                let dropped: Vec<String> = deps
                    .iter()
                    .filter(|d| is_self(d))
                    .map(|d| d.debian_name.clone())
                    .collect();
                deps.retain(|d| !is_self(d));
                for name in dropped {
                    notes.push(format!(
                        "{:?} '{}': dropped self-reference",
                        dep_type, name
                    ));
                }
            }
        }

        notes
    }

    /// Resolve a single dependency
    ///
    /// Candidates are collected from every source (local DB, virtual
    /// providers, regex rules, fuzzy matching, AUR) and the highest
    /// confidence mapping wins. The previous first-match-wins order let a
    /// low-confidence regex guess shadow an exact AUR hit.
    async fn resolve_single(&self, dep: &mut Dependency) -> Result<()> {
        // Skip if already resolved
        if dep.is_mapped() {
            return Ok(());
        }

        let mut best: Option<(String, f32)> = None;

        // 1. Exact mapping from local DB
        consider_best(&mut best, self.db.lookup(&dep.debian_name)?);

        // 2. Known virtual package: map to its preferred provider
        if self.db.is_virtual(&dep.debian_name)? {
            dep.is_virtual = true;
            if let Some(providers) = self.db.get_virtual_providers(&dep.debian_name) {
                if let Some(provider) = providers.first() {
                    consider_best(&mut best, Some((provider.clone(), 0.85)));
                }
            }
        }

        // 3. Rule-based mapping via PackageMapper (regex transformations)
        if let Ok(mut mapper) = self.mapper.lock() {
            consider_best(&mut best, mapper.apply_rules(&dep.debian_name));
        }

        // 4. Fuzzy matching against local DB
        consider_best(&mut best, self.fuzzy.find_best_match(&dep.debian_name, &self.db)?);

        // 5. AUR exact name match (skipped when we already have better)
        let needs_aur = best.as_ref().map_or(true, |(_, c)| *c < 1.0);
        if needs_aur {
            if let Ok(results) = self.aur.info(&[&dep.debian_name]).await {
                if let Some(pkg) = results.first() {
                    consider_best(&mut best, Some((pkg.name.clone(), 1.0)));
                }
            }
        }

        // 6. AUR provider search (for virtual packages or libraries)
        let needs_providers = best.as_ref().map_or(true, |(_, c)| *c < 0.9);
        if needs_providers {
            if let Ok(providers) = self.aur.find_providers(&dep.debian_name).await {
                if let Some(pkg) = providers.first() {
                    let confidence = if pkg.name == dep.debian_name { 1.0 } else { 0.8 };
                    consider_best(&mut best, Some((pkg.name.clone(), confidence)));
                }
            }
        }

        if let Some((arch_name, confidence)) = best {
            dep.set_arch_name(arch_name, confidence);
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
                        jre_deps.push((*dep_type, dep.debian_name.clone(), arch_name.clone()));
                    }
                    if java_jdk_patterns.iter().any(|p| arch_name.contains(p)) {
                        jdk_deps.push((*dep_type, dep.debian_name.clone(), arch_name.clone()));
                    }
                }
            }
        }

        // If both JRE and JDK dependencies exist, keep one side based on strategy.
        // (A JDK provides a full runtime, so dropping the JRE side is safe.
        // No `conflicts=` entries are added: jre-*/jdk-* packages coexist
        // fine on Arch, and a bogus conflict would break installation for
        // users who already have a JRE.)
        if !jre_deps.is_empty() && !jdk_deps.is_empty() {
            let keep_jdk = match config.java.conflict_strategy.as_str() {
                "prefer-jdk" | "jdk" => true,
                "prefer-jre" | "jre" => false,
                "prompt" => prompt_java_choice(&jre_deps, &jdk_deps, config.general.auto_yes),
                _ => true,
            };
            if keep_jdk {
                remove_java_deps(metadata, &jre_deps);
            } else {
                remove_java_deps(metadata, &jdk_deps);
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

/// Remove the given `(dep_type, debian_name, _)` entries from the metadata
fn remove_java_deps(
    metadata: &mut PackageMetadata,
    entries: &[(DependencyType, String, String)],
) {
    for (dep_type, debian_name, _) in entries {
        if let Some(deps) = metadata.dependencies.get_mut(dep_type) {
            deps.retain(|dep| dep.debian_name != *debian_name);
        }
    }
}

/// Ask the user which Java stack to keep; returns `true` for JDK
///
/// Falls back to JDK (the safe default) when `auto_yes` is set, when there
/// is no interactive terminal, or when the prompt is cancelled.
fn prompt_java_choice(
    jre_deps: &[(DependencyType, String, String)],
    jdk_deps: &[(DependencyType, String, String)],
    auto_yes: bool,
) -> bool {
    if auto_yes || !console::user_attended() {
        return true;
    }
    let jre_list: Vec<&str> = jre_deps.iter().map(|(_, _, a)| a.as_str()).collect();
    let jdk_list: Vec<&str> = jdk_deps.iter().map(|(_, _, a)| a.as_str()).collect();
    println!(
        "Both JRE ({}) and JDK ({}) dependencies were found.",
        jre_list.join(", "),
        jdk_list.join(", ")
    );
    let choice = dialoguer::Select::new()
        .with_prompt("Which Java stack should the converted package depend on?")
        .items(&["JDK (recommended, includes a runtime)", "JRE"])
        .default(0)
        .interact_opt();
    !matches!(choice, Ok(Some(1)))
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
