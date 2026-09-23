//! Interactive prompts for the CLI
//!
//! Picker, dependency review, and configuration wizards. All prompts are
//! gated behind [`is_promptable`] so scripts, pipes, `--yes`, and `--quiet`
//! never block waiting for input.

use std::path::PathBuf;

use crate::error::{RexebError, Result};
use crate::models::{DependencyType, PackageMetadata};

/// Whether interactive prompts may be shown
///
/// True on an attended terminal unless `--yes`/`--quiet` (or the config
/// `auto_yes`) asked for non-interactive behavior.
pub fn is_promptable(yes: bool, quiet: bool) -> bool {
    if yes || quiet {
        return false;
    }
    if crate::config::Config::load()
        .map(|c| c.general.auto_yes)
        .unwrap_or(false)
    {
        return false;
    }
    console::user_attended()
}

/// Whether a path looks like a convertible package (extension check)
pub fn is_package_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "deb" | "rpm" | "appimage"
            )
        })
        .unwrap_or(false)
}

/// Pick package files (`.deb`, `.rpm`, `.AppImage`) from the current directory
///
/// Shows every convertible file in the working directory in a multi-select;
/// when nothing is selected (or nothing exists), falls back to validated
/// manual path entry.
pub fn pick_input_files() -> Result<Vec<PathBuf>> {
    use dialoguer::{Input, MultiSelect};

    let mut packages: Vec<PathBuf> = std::fs::read_dir(".")?
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_package_file(p))
        .collect();
    packages.sort();

    if packages.is_empty() {
        println!("No .deb, .rpm, or .AppImage files in the current directory.");
    } else {
        let labels: Vec<String> = packages
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        let picked = MultiSelect::new()
            .with_prompt("Select packages to convert (Space to toggle, Enter to confirm)")
            .items(&labels)
            .interact()?;
        let selected: Vec<PathBuf> = picked.into_iter().map(|i| packages[i].clone()).collect();
        if !selected.is_empty() {
            return Ok(selected);
        }
        println!("Nothing selected.");
    }

    loop {
        let path: String = Input::new()
            .with_prompt("Path to a package file (.deb, .rpm, .AppImage; empty aborts)")
            .allow_empty(true)
            .interact_text()?;
        let path = path.trim();
        if path.is_empty() {
            return Err(RexebError::Validation("No input files given".into()));
        }
        let candidate = PathBuf::from(path);
        if !candidate.exists() {
            println!("'{}' does not exist — try again.", path);
            continue;
        }
        if !is_package_file(&candidate) {
            println!(
                "'{}' is not a .deb, .rpm, or .AppImage file — try again.",
                path
            );
            continue;
        }
        return Ok(vec![candidate]);
    }
}

/// Review unmapped hard dependencies with the user
///
/// For every unmapped `Depends`/`PreDepends` entry the user may type the
/// Arch package name, search the AUR, drop the dependency, or abort the
/// conversion. Mappings created this way are saved to the database so the
/// next conversion of a similar package needs no prompting.
pub async fn review_unmapped_dependencies(metadata: &mut PackageMetadata) -> Result<()> {
    use console::style;
    use dialoguer::{FuzzySelect, Input, Select};

    let hard = [DependencyType::Depends, DependencyType::PreDepends];
    let mut learned: Vec<(String, String)> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();

    loop {
        // Re-scan each round: dropping an entry shifts indices
        let next = hard.iter().find_map(|t| {
            metadata
                .get_deps(*t)
                .iter()
                .position(|d| !d.is_mapped())
                .map(|i| (*t, i))
        });
        let Some((dep_type, idx)) = next else {
            break;
        };
        let (debian_name, version) = {
            let dep = &metadata.get_deps(dep_type)[idx];
            (dep.debian_name.clone(), dep.version.clone())
        };

        println!(
            "\n{} Unmapped {:?} dependency: {}{}",
            style("?").yellow().bold(),
            dep_type,
            style(&debian_name).cyan(),
            version
                .map(|v| format!(" (version {})", v))
                .unwrap_or_default(),
        );
        let options = [
            "Type the Arch package name",
            "Search the AUR",
            "Drop this dependency",
            "Abort conversion",
        ];
        let choice = Select::new()
            .with_prompt("How to resolve it?")
            .items(&options)
            .default(0)
            .interact_opt()?;

        match choice {
            Some(0) => {
                let name: String = Input::new()
                    .with_prompt("Arch package name")
                    .interact_text()?;
                let name = name.trim().to_string();
                if name.is_empty()
                    || name.contains(|c: char| c.is_whitespace() || c == '/')
                {
                    println!("Invalid package name — try again.");
                    continue;
                }
                if let Some(deps) = metadata.dependencies.get_mut(&dep_type) {
                    if let Some(dep) = deps.get_mut(idx) {
                        dep.set_arch_name(name.clone(), 1.0);
                    }
                }
                learned.push((debian_name, name));
            }
            Some(1) => {
                let client = crate::resolver::AurClient::new();
                let results = client.search(&debian_name).await.unwrap_or_default();
                if results.is_empty() {
                    println!("No AUR results for '{}'.", debian_name);
                    continue;
                }
                let labels: Vec<String> = results
                    .iter()
                    .take(20)
                    .map(|p| {
                        format!(
                            "{} {} — {}",
                            p.name,
                            p.version,
                            p.description.as_deref().unwrap_or("(no description)")
                        )
                    })
                    .collect();
                let picked = FuzzySelect::new()
                    .with_prompt("Pick a package (Esc to go back)")
                    .items(&labels)
                    .interact_opt()?;
                match picked {
                    Some(i) => {
                        let chosen = results[i].name.clone();
                        if let Some(deps) = metadata.dependencies.get_mut(&dep_type) {
                            if let Some(dep) = deps.get_mut(idx) {
                                dep.set_arch_name(chosen.clone(), 0.9);
                            }
                        }
                        learned.push((debian_name, chosen));
                    }
                    None => continue,
                }
            }
            Some(2) => {
                if let Some(deps) = metadata.dependencies.get_mut(&dep_type) {
                    if idx < deps.len() {
                        deps.remove(idx);
                    }
                }
                dropped.push(debian_name);
            }
            _ => {
                return Err(RexebError::Validation("Conversion aborted by user".into()));
            }
        }
    }

    if !learned.is_empty() {
        let mut db = crate::resolver::PackageDatabase::new()?;
        for (deb, arch) in &learned {
            db.add_mapping(deb, arch, 1.0);
        }
        db.save()?;
        println!(
            "{} Saved {} new mapping(s) to the database.",
            style("✓").green(),
            learned.len()
        );
    }
    if !dropped.is_empty() {
        println!("{} Dropped: {}", style("!").yellow(), dropped.join(", "));
    }
    Ok(())
}

/// Yes/no confirmation (defaults to yes)
pub fn confirm_proceed(prompt: &str) -> Result<bool> {
    use dialoguer::Confirm;
    Ok(Confirm::new()
        .with_prompt(prompt)
        .default(true)
        .interact_opt()?
        .unwrap_or(false))
}

/// Prompt for the `map add` values that were not given on the command line
pub fn prompt_map_add(
    debian: Option<String>,
    arch: Option<String>,
    confidence: f32,
) -> Result<(String, String, f32)> {
    use dialoguer::Input;

    let asked = debian.is_none() || arch.is_none();
    let debian = match debian {
        Some(d) => d,
        None => Input::new()
            .with_prompt("Debian package name")
            .interact_text()
            .map(|s: String| s.trim().to_string())?,
    };
    if debian.trim().is_empty() {
        return Err(RexebError::Validation(
            "Debian package name is required".into(),
        ));
    }
    let arch = match arch {
        Some(a) => a,
        None => Input::new()
            .with_prompt("Arch package name")
            .interact_text()
            .map(|s: String| s.trim().to_string())?,
    };
    if arch.trim().is_empty() {
        return Err(RexebError::Validation(
            "Arch package name is required".into(),
        ));
    }

    // Only ask about confidence when we already prompted for a name;
    // fully-specified CLI invocations keep the (validated) flag value.
    let confidence = if asked {
        let raw: String = Input::new()
            .with_prompt("Confidence (0.0 - 1.0)")
            .with_initial_text("1.0")
            .interact_text()?;
        match raw.trim().parse::<f32>() {
            Ok(v) if v.is_finite() => v.clamp(0.0, 1.0),
            _ => {
                println!("Invalid confidence — using 1.0.");
                1.0
            }
        }
    } else {
        confidence
    };
    Ok((debian, arch, confidence))
}

/// Interactive `config init` wizard asking for the main settings
pub fn prompt_config_wizard() -> Result<crate::config::Config> {
    use dialoguer::{Confirm, Input, Select};

    let mut config = crate::config::Config::default();

    let output: String = Input::new()
        .with_prompt("Default output directory (empty = current directory)")
        .allow_empty(true)
        .interact_text()?;
    if !output.trim().is_empty() {
        config.general.output_dir = Some(PathBuf::from(output.trim()));
    }

    let jobs: String = Input::new()
        .with_prompt("Parallel jobs (empty = auto)")
        .allow_empty(true)
        .interact_text()?;
    if !jobs.trim().is_empty() {
        match jobs.trim().parse::<usize>() {
            Ok(j) if j >= 1 => config.general.jobs = Some(j),
            _ => println!("Invalid number — using auto."),
        }
    }

    let formats = ["pkg.tar.zst (recommended)", "pkg.tar.xz", "pkg.tar.gz"];
    let fmt_idx = Select::new()
        .with_prompt("Default output format")
        .items(&formats)
        .default(0)
        .interact_opt()?
        .unwrap_or(0);
    config.conversion.default_format = ["pkg.tar.zst", "pkg.tar.xz", "pkg.tar.gz"][fmt_idx].to_string();

    config.conversion.strip_binaries = Confirm::new()
        .with_prompt("Strip binaries after staging?")
        .default(false)
        .interact_opt()?
        .unwrap_or(false);
    config.conversion.skip_deps = Confirm::new()
        .with_prompt("Skip dependency resolution by default?")
        .default(false)
        .interact_opt()?
        .unwrap_or(false);

    let timeout: String = Input::new()
        .with_prompt("Network timeout in seconds")
        .with_initial_text("30")
        .interact_text()?;
    match timeout.trim().parse::<u64>() {
        Ok(t) if t >= 1 => config.network.timeout = t,
        _ => println!("Invalid number — using 30."),
    }

    Ok(config)
}
