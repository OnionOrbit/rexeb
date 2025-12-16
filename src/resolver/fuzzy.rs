//! Fuzzy matching for package names

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher as FuzzyMatcherTrait;
use strsim::{jaro_winkler, normalized_damerau_levenshtein};

use crate::error::Result;

use super::PackageDatabase;

/// Fuzzy matcher for finding similar package names
pub struct FuzzyMatcher {
    /// Skim fuzzy matcher
    skim: SkimMatcherV2,
    /// Minimum score threshold
    min_score: f32,
}

impl FuzzyMatcher {
    /// Create a new fuzzy matcher
    pub fn new() -> Self {
        Self {
            skim: SkimMatcherV2::default(),
            min_score: 0.6,
        }
    }

    /// Set the minimum score threshold
    pub fn with_min_score(mut self, score: f32) -> Self {
        self.min_score = score;
        self
    }

    /// Find the best matching Arch package for a Debian package name
    pub fn find_best_match(&self, debian_name: &str, db: &PackageDatabase) -> Result<Option<(String, f32)>> {
        let candidates = db.get_arch_package_names();
        
        if candidates.is_empty() {
            // Fall back to heuristic matching
            return Ok(self.heuristic_match(debian_name));
        }

        let mut best_match: Option<(String, f32)> = None;

        for candidate in candidates {
            let score = self.calculate_score(debian_name, candidate);
            
            if score >= self.min_score {
                if best_match.as_ref().map_or(true, |(_, s)| score > *s) {
                    best_match = Some((candidate.to_string(), score));
                }
            }
        }

        // Also try heuristic matching
        if let Some((heuristic_name, heuristic_score)) = self.heuristic_match(debian_name) {
            if best_match.as_ref().map_or(true, |(_, s)| heuristic_score > *s) {
                best_match = Some((heuristic_name, heuristic_score));
            }
        }

        Ok(best_match)
    }

    /// Calculate similarity score between two package names
    fn calculate_score(&self, debian_name: &str, arch_name: &str) -> f32 {
        // Normalize names for comparison
        let norm_debian = self.normalize_name(debian_name);
        let norm_arch = self.normalize_name(arch_name);

        // If normalized names are equal, high score
        if norm_debian == norm_arch {
            return 0.95;
        }

        // Use multiple algorithms and combine scores
        let mut scores = Vec::new();

        // Skim fuzzy matching
        if let Some(skim_score) = self.skim.fuzzy_match(&norm_debian, &norm_arch) {
            // Normalize skim score (typically 0-100+)
            let normalized = (skim_score as f32 / 100.0).min(1.0);
            scores.push(normalized);
        }

        // Jaro-Winkler similarity (good for typos and prefixes)
        let jw_score = jaro_winkler(&norm_debian, &norm_arch) as f32;
        scores.push(jw_score);

        // Normalized Damerau-Levenshtein (handles transpositions)
        let ndl_score = normalized_damerau_levenshtein(&norm_debian, &norm_arch) as f32;
        scores.push(ndl_score);

        // Check for common patterns
        let pattern_score = self.pattern_match(&norm_debian, &norm_arch);
        if pattern_score > 0.0 {
            scores.push(pattern_score);
        }

        // Calculate weighted average
        if scores.is_empty() {
            return 0.0;
        }

        let sum: f32 = scores.iter().sum();
        sum / scores.len() as f32
    }

    /// Normalize a package name for comparison
    fn normalize_name(&self, name: &str) -> String {
        let mut normalized = name.to_lowercase();

        // Remove common prefixes
        for prefix in &["lib", "python3-", "python-", "perl-", "ruby-", "node-", "golang-"] {
            if normalized.starts_with(prefix) {
                // Keep the prefix info but strip it for comparison
                normalized = normalized[prefix.len()..].to_string();
                break;
            }
        }

        // Remove common suffixes
        for suffix in &["-dev", "-dbg", "-doc", "-common", "-data", "-bin", "-utils"] {
            if normalized.ends_with(suffix) {
                normalized = normalized[..normalized.len() - suffix.len()].to_string();
                break;
            }
        }

        // Remove version numbers at the end (e.g., libfoo6 -> libfoo)
        let mut chars: Vec<char> = normalized.chars().collect();
        while chars.last().map_or(false, |c| c.is_ascii_digit()) {
            chars.pop();
        }
        normalized = chars.into_iter().collect();

        // Remove hyphens and underscores for comparison
        normalized = normalized.replace(['-', '_'], "");

        normalized
    }

    /// Check for common package naming patterns
    fn pattern_match(&self, debian: &str, arch: &str) -> f32 {
        // Common transformations
        let transformations = [
            // Debian lib*N -> Arch lib*
            (r"^lib(.+)\d+$", "lib$1"),
            // python3-* -> python-*
            (r"^python3-(.+)$", "python-$1"),
            // *-dev -> *-devel (though Arch usually uses headers)
            (r"^(.+)-dev$", "$1-devel"),
        ];

        for (pattern, replacement) in transformations {
            if let Ok(re) = regex::Regex::new(pattern) {
                if re.is_match(debian) {
                    let transformed = re.replace(debian, replacement);
                    if transformed == arch {
                        return 0.85;
                    }
                }
            }
        }

        // Check if one is a substring of the other
        if debian.contains(arch) || arch.contains(debian) {
            let len_ratio = debian.len().min(arch.len()) as f32 / debian.len().max(arch.len()) as f32;
            return len_ratio * 0.7;
        }

        0.0
    }

    /// Heuristic matching for common Debian -> Arch patterns
    fn heuristic_match(&self, debian_name: &str) -> Option<(String, f32)> {
        let name = debian_name.to_lowercase();

        // lib*N -> lib* (remove version number)
        if name.starts_with("lib") {
            let mut stripped = name.clone();
            while stripped.chars().last().map_or(false, |c| c.is_ascii_digit()) {
                stripped.pop();
            }
            if stripped != name && stripped.len() > 3 {
                return Some((stripped, 0.7));
            }
        }

        // python3-* -> python-*
        if let Some(rest) = name.strip_prefix("python3-") {
            return Some((format!("python-{}", rest), 0.8));
        }

        // lib*-dev -> * (development files)
        if name.starts_with("lib") && name.ends_with("-dev") {
            let core = &name[3..name.len() - 4];
            // Remove version number
            let mut core_clean: String = core.chars().take_while(|c| !c.is_ascii_digit()).collect();
            if core_clean.ends_with('-') {
                core_clean.pop();
            }
            if !core_clean.is_empty() {
                return Some((format!("lib{}", core_clean), 0.65));
            }
        }

        // *-dev -> * (development files in Arch are usually in main package)
        if let Some(core) = name.strip_suffix("-dev") {
            return Some((core.to_string(), 0.6));
        }

        // Common renamings
        let common_renames = [
            ("apt", "pacman"),
            ("dpkg", "pacman"),
            ("default-jre", "jre-openjdk"),
            ("default-jdk", "jdk-openjdk"),
            ("openjdk-11-jre", "jre11-openjdk"),
            ("openjdk-17-jre", "jre17-openjdk"),
            ("libreoffice-core", "libreoffice-fresh"),
        ];

        for (deb, arch) in common_renames {
            if name == deb {
                return Some((arch.to_string(), 0.9));
            }
        }

        None
    }

    /// Find multiple matches sorted by score
    pub fn find_matches(&self, debian_name: &str, db: &PackageDatabase, limit: usize) -> Result<Vec<(String, f32)>> {
        let candidates = db.get_arch_package_names();
        let mut matches: Vec<(String, f32)> = Vec::new();

        for candidate in candidates {
            let score = self.calculate_score(debian_name, candidate);
            if score >= self.min_score {
                matches.push((candidate.to_string(), score));
            }
        }

        // Add heuristic matches
        if let Some(heuristic) = self.heuristic_match(debian_name) {
            // Only add if not already in matches
            if !matches.iter().any(|(n, _)| n == &heuristic.0) {
                matches.push(heuristic);
            }
        }

        // Sort by score descending
        matches.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Limit results
        matches.truncate(limit);

        Ok(matches)
    }
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_name() {
        let matcher = FuzzyMatcher::new();
        
        assert_eq!(matcher.normalize_name("libfoo6"), "foo");
        assert_eq!(matcher.normalize_name("python3-bar"), "bar");
        assert_eq!(matcher.normalize_name("baz-dev"), "baz");
    }

    #[test]
    fn test_heuristic_match() {
        let matcher = FuzzyMatcher::new();
        
        let result = matcher.heuristic_match("python3-numpy");
        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "python-numpy");
    }

    #[test]
    fn test_score_calculation() {
        let matcher = FuzzyMatcher::new();
        
        // Exact match after normalization should be high
        let score = matcher.calculate_score("libfoo6", "libfoo");
        assert!(score > 0.8);
        
        // Completely different should be low
        let score = matcher.calculate_score("firefox", "chromium");
        assert!(score < 0.5);
    }
}