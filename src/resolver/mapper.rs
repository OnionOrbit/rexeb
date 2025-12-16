//! Package name mapper with additional intelligence

use std::collections::HashMap;

use crate::error::Result;
use crate::models::Dependency;

/// Intelligent package name mapper
pub struct PackageMapper {
    /// Custom mapping rules
    rules: Vec<MappingRule>,
    /// Cached mappings
    cache: HashMap<String, Option<String>>,
}

/// A mapping rule
#[derive(Debug, Clone)]
pub struct MappingRule {
    /// Rule name for debugging
    pub name: String,
    /// Pattern to match (regex)
    pub pattern: regex::Regex,
    /// Replacement template
    pub replacement: String,
    /// Confidence for this rule
    pub confidence: f32,
}

impl PackageMapper {
    /// Create a new package mapper
    pub fn new() -> Self {
        let mut mapper = Self {
            rules: Vec::new(),
            cache: HashMap::new(),
        };
        mapper.load_default_rules();
        mapper
    }

    /// Load default mapping rules
    fn load_default_rules(&mut self) {
        let rules = [
            // Library versioning: libfoo6 -> libfoo
            ("lib-version", r"^lib(.+?)(\d+)$", "lib$1", 0.8),
            
            // Python packages: python3-foo -> python-foo
            ("python3", r"^python3-(.+)$", "python-$1", 0.9),
            
            // Perl modules: libfoo-perl -> perl-foo
            ("perl-lib", r"^lib(.+)-perl$", "perl-$1", 0.85),
            
            // Ruby gems: ruby-foo -> ruby-foo
            ("ruby", r"^ruby-(.+)$", "ruby-$1", 0.9),
            
            // Node.js: node-foo -> nodejs-foo
            ("node", r"^node-(.+)$", "nodejs-$1", 0.85),
            
            // Development files: libfoo-dev -> foo
            ("dev-files", r"^lib(.+)-dev$", "$1", 0.6),
            
            // DBG packages: foo-dbg -> foo-debug
            ("debug", r"^(.+)-dbg$", "$1-debug", 0.7),
            
            // Documentation: foo-doc -> foo-docs
            ("docs", r"^(.+)-doc$", "$1-docs", 0.8),
            
            // GTK themes and icons
            ("gtk-theme", r"^(.+)-theme-(.+)$", "$1-$2-theme", 0.75),
            
            // Fonts: fonts-foo -> ttf-foo or otf-foo
            ("fonts", r"^fonts-(.+)$", "ttf-$1", 0.7),
            
            // GStreamer plugins: gstreamer1.0-foo -> gst-plugins-foo
            ("gstreamer", r"^gstreamer1\.0-(.+)$", "gst-plugins-$1", 0.85),
            
            // Typelib bindings: gir1.2-foo -> foo
            ("typelib", r"^gir1\.2-(.+)-[\d.]+$", "$1", 0.6),
            
            // Qt5 libraries: libqt5foo5 -> qt5-base (generic)
            ("qt5-lib", r"^libqt5(.+)\d+$", "qt5-$1", 0.7),
            
            // Qt6 libraries: libqt6foo6 -> qt6-base (generic)
            ("qt6-lib", r"^libqt6(.+)\d+$", "qt6-$1", 0.7),
            
            // Boost libraries: libboost-foo1.74.0 -> boost-libs
            ("boost", r"^libboost-(.+?)[\d.]+$", "boost-libs", 0.75),
            
            // ICU libraries: libicu* -> icu
            ("icu", r"^libicu(.+)\d+$", "icu", 0.8),
            
            // LLVM: libllvm14 -> llvm-libs
            ("llvm", r"^libllvm\d+$", "llvm-libs", 0.9),
            
            // Clang: libclang1-14 -> clang
            ("clang", r"^libclang\d+-\d+$", "clang", 0.9),
        ];

        for (name, pattern, replacement, confidence) in rules {
            if let Ok(regex) = regex::Regex::new(pattern) {
                self.rules.push(MappingRule {
                    name: name.to_string(),
                    pattern: regex,
                    replacement: replacement.to_string(),
                    confidence,
                });
            }
        }
    }

    /// Apply mapping rules to a package name
    pub fn apply_rules(&mut self, debian_name: &str) -> Option<(String, f32)> {
        // Check cache first
        if let Some(cached) = self.cache.get(debian_name) {
            if let Some(cached_name) = cached {
                // We need to find the original confidence for this mapping
                // For now, let's apply the rule again to get the confidence
                for rule in &self.rules {
                    if rule.pattern.is_match(debian_name) {
                        return Some((cached_name.clone(), rule.confidence));
                    }
                }
            }
            return None;
        }

        for rule in &self.rules {
            if rule.pattern.is_match(debian_name) {
                let mapped = rule.pattern.replace(debian_name, &rule.replacement);
                let result = Some((mapped.to_string(), rule.confidence));
                self.cache.insert(debian_name.to_string(), Some(mapped.to_string()));
                return result;
            }
        }

        self.cache.insert(debian_name.to_string(), None);
        None
    }

    /// Get a suggested Arch package name for a Debian dependency
    pub fn suggest_arch_name(&mut self, dep: &Dependency) -> Option<(String, f32)> {
        // Try rule-based mapping
        if let Some(result) = self.apply_rules(&dep.debian_name) {
            return Some(result);
        }

        // Try simple transformations
        self.simple_transform(&dep.debian_name)
    }

    /// Simple transformations for common patterns
    fn simple_transform(&self, name: &str) -> Option<(String, f32)> {
        let name_lower = name.to_lowercase();

        // Same name might work
        if !name_lower.contains("debian") 
            && !name_lower.contains("ubuntu")
            && !name_lower.starts_with("lib")
        {
            return Some((name_lower, 0.5));
        }

        // Strip lib prefix and version suffix
        if name_lower.starts_with("lib") {
            let mut base = name_lower[3..].to_string();
            
            // Remove trailing numbers
            while base.chars().last().map_or(false, |c| c.is_ascii_digit()) {
                base.pop();
            }
            
            if !base.is_empty() {
                return Some((format!("lib{}", base), 0.55));
            }
        }

        None
    }

    /// Add a custom rule
    pub fn add_rule(&mut self, name: &str, pattern: &str, replacement: &str, confidence: f32) -> Result<()> {
        let regex = regex::Regex::new(pattern)?;
        self.rules.push(MappingRule {
            name: name.to_string(),
            pattern: regex,
            replacement: replacement.to_string(),
            confidence,
        });
        // Clear cache when rules change
        self.cache.clear();
        Ok(())
    }

    /// Clear the cache
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

impl Default for PackageMapper {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lib_version_rule() {
        let mut mapper = PackageMapper::new();
        
        let result = mapper.apply_rules("libpng16");
        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "libpng");
    }

    #[test]
    fn test_python3_rule() {
        let mut mapper = PackageMapper::new();
        
        let result = mapper.apply_rules("python3-numpy");
        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "python-numpy");
    }

    #[test]
    fn test_dev_files_rule() {
        let mut mapper = PackageMapper::new();
        
        let result = mapper.apply_rules("libssl-dev");
        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "ssl");
    }

    #[test]
    fn test_caching() {
        let mut mapper = PackageMapper::new();
        
        // First call
        let result1 = mapper.apply_rules("python3-test");
        assert!(result1.is_some());
        
        // Second call should use cache
        let result2 = mapper.apply_rules("python3-test");
        assert_eq!(result1, result2);
    }
}
