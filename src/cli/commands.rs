//! Command execution handlers

use std::path::Path;

use crate::error::Result;

/// Execute the convert command
pub async fn execute_convert(args: &super::ConvertArgs) -> Result<()> {
    use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

    let multi = MultiProgress::new();
    let style = ProgressStyle::with_template(
        "{prefix:.bold.dim} [{bar:40.cyan/blue}] {pos}/{len} {msg}"
    )
    .unwrap()
    .progress_chars("█▓▒░ ");

    let output_dir = args.output.clone().unwrap_or_else(|| std::env::current_dir().unwrap());
    
    // Process packages using tasks since we're async now
    let mut handles = Vec::new();
    
    for input_path in &args.input {
        let input_path = input_path.clone();
        let output_dir = output_dir.clone();
        let args_clone = args.clone();
        
        let pb = multi.add(ProgressBar::new(100));
        pb.set_style(style.clone());
        pb.set_prefix(format!("{}", input_path.file_name().unwrap_or_default().to_string_lossy()));
        
        handles.push(tokio::spawn(async move {
            convert_single_package(&input_path, &output_dir, &args_clone, pb).await
        }));
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.map_err(|e| crate::error::RexebError::Other(e.to_string()))??;
    }

    Ok(())
}

/// Convert a single package
async fn convert_single_package(
    input: &Path,
    output_dir: &Path,
    args: &super::ConvertArgs,
    pb: indicatif::ProgressBar,
) -> Result<()> {
    use crate::converter::PackageConverter;
    use crate::parsers::deb::DebParser;

    pb.set_message("Parsing package...");
    pb.set_position(10);

    // Parse the deb package
    let parser = DebParser::new(input)?;
    let mut metadata = parser.parse()?;

    pb.set_position(30);
    pb.set_message("Resolving dependencies...");

    // Apply overrides
    if let Some(ref name) = args.name {
        metadata.arch_name = Some(name.clone());
    }
    if let Some(ref version) = args.version_override {
        metadata.version = version.clone();
    }
    if let Some(ref release) = args.release {
        metadata.release = release.clone();
    }

    // Normalize version
    metadata.normalize_version();

    pb.set_position(40);

    // Resolve dependencies if not skipped
    let resolver = crate::resolver::DependencyResolver::new()?;
    if !args.skip_deps {
        resolver.resolve(&mut metadata).await?;
    }

    // Show resolution stats
    let stats = resolver.stats(&metadata);
    if stats.total > 0 {
        pb.set_prefix(format!(
            "{} [{}/{} mapped, {:.0}%]",
            input.file_name().unwrap_or_default().to_string_lossy(),
            stats.mapped,
            stats.total,
            stats.success_rate() * 100.0
        ));
    }

    pb.set_position(60);
    pb.set_message("Building package...");

    // Create output package
    if args.pkgbuild {
        // Generate PKGBUILD
        let pkgbuild_path = output_dir.join("PKGBUILD");
        std::fs::write(&pkgbuild_path, metadata.to_pkgbuild())?;
        pb.set_position(100);
        pb.finish_with_message(format!("Created {}", pkgbuild_path.display()));
    } else {
        // Build binary package
        let mut converter = PackageConverter::new(metadata, parser.extract_dir())?;

        // Optionally enable sandboxed build
        if args.sandbox {
            let sandbox_root = tempfile::TempDir::new()?.keep();
            converter = converter.with_sandbox(&sandbox_root)?;
            pb.set_message("Building in sandbox...");
        }

        let output_path = converter.build(output_dir, args.format)?;
        pb.set_position(100);
        pb.finish_with_message(format!("Created {}", output_path.display()));
    }

    Ok(())
}

/// Execute the update command
pub async fn execute_update(args: &super::UpdateArgs) -> Result<()> {
    use crate::resolver::database::PackageDatabase;
    use indicatif::{ProgressBar, ProgressStyle};

    let style = ProgressStyle::with_template(
        "{spinner:.green} [{bar:40.cyan/blue}] {msg}"
    )
    .unwrap();

    let pb = ProgressBar::new_spinner();
    pb.set_style(style.clone());

    let update_all = args.all || (!args.virtual_packages && !args.mappings && !args.aur);

    let mut db = PackageDatabase::new()?;

    if update_all || args.mappings {
        pb.set_message("Updating package mappings...");
        db.update_mappings(args.force).await?;
    }

    if update_all || args.virtual_packages {
        pb.set_message("Updating virtual packages database...");
        db.update_virtual_packages(args.force).await?;
    }

    if update_all || args.aur {
        pb.set_message("Updating AUR cache...");
        db.update_aur_cache(args.force).await?;
    }

    // Enlarge: crawl local sync DBs and optionally debtap mappings
    if args.enlarge {
        pb.set_message("Enlarging mappings from local repo DBs...");
        let added = enlarge_mappings(&mut db).await?;
        if added > 0 {
            println!("Enlarged: {} new mappings proposed from local DB scan", added);
        } else {
            println!("No new mappings found during enlarge scan");
        }
    }

    pb.finish_with_message("Database updated successfully");
    Ok(())
}

/// Enlarge mappings by scanning pacman sync DBs and proposing new entries
async fn enlarge_mappings(db: &mut crate::resolver::database::PackageDatabase) -> Result<usize> {
    use std::collections::HashMap;
    use std::io::Read;
    use std::path::Path;

    // Scan Arch sync DBs to harvest all package names + provides
    let sync_dir = Path::new("/var/lib/pacman/sync");
    let mut arch_names: Vec<String> = Vec::new();
    let mut provides_map: HashMap<String, String> = HashMap::new();

    if sync_dir.exists() {
        for entry in std::fs::read_dir(sync_dir).map_err(|e| crate::error::RexebError::Other(e.to_string()))? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("db") {
                continue;
            }
            if let Ok(file) = std::fs::File::open(&path) {
                let decoder = flate2::read::GzDecoder::new(file);
                let mut archive = tar::Archive::new(decoder);
                for ent in archive.entries().into_iter().flatten().flatten() {
                    let path_in = ent.path().unwrap_or_default().to_path_buf();
                    let comps: Vec<_> = path_in.components().collect();
                    if comps.len() < 2 || comps[1].as_os_str().to_string_lossy() != "desc" {
                        continue;
                    }
                    let mut content = String::new();
                    let mut r = ent;
                    let _ = r.read_to_string(&mut content);
                    // Extract NAME
                    if let Some(start) = content.find("%NAME%") {
                        let rest = &content[start + 6..];
                        let name = rest.lines().skip(1).next().unwrap_or("").trim().to_string();
                        if !name.is_empty() {
                            arch_names.push(name.clone());
                            // Extract PROVIDES for this package
                            if let Some(p) = content.find("%PROVIDES%") {
                                let pr = &content[p + 10..];
                                for prov in pr.lines().take_while(|l| !l.starts_with('%')).map(|l| l.trim().to_string()).filter(|l| !l.is_empty()) {
                                    provides_map.entry(prov).or_insert_with(|| name.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Debtap mapping import (if reachable)
    let debtap_url = "https://raw.githubusercontent.com/helixarch/debtap/master/debtap";
    let mut debtap_mappings: HashMap<String, String> = HashMap::new();
    if let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        if let Ok(resp) = client.get(debtap_url).send().await {
            if resp.status().is_success() {
                if let Ok(text) = resp.text().await {
                    // debtap mappings look like: "pkg-deb" => "pkg-arch"
                    let re = regex::Regex::new(r#""([^"]+)"\s*=>\s*"([^"]+)""#).unwrap();
                    for cap in re.captures_iter(&text).take(2000) {
                        let deb = cap[1].to_string();
                        let arch = cap[2].to_string();
                        if !deb.is_empty() && !arch.is_empty() {
                            debtap_mappings.insert(deb, arch);
                        }
                    }
                    tracing::info!("Parsed {} debtap mappings", debtap_mappings.len());
                }
            }
        }
    }

    // Merge: for each debtap or provides entry not in DB, propose with confidence 0.7-0.8
    let mut added = 0;
    for (deb, arch) in debtap_mappings.iter().chain(provides_map.iter()) {
        if db.lookup(deb).unwrap_or(None).is_none() {
            // Validate arch name looks plausible
            if arch.is_empty() || arch.contains('/') || arch.contains(' ') {
                continue;
            }
            db.add_mapping(deb, arch, 0.75);
            added += 1;
        }
    }

    if added > 0 {
        db.save()?;
    }

    tracing::info!("Enlarge finished: {} new mappings, scanned {} arch packages", added, arch_names.len());
    Ok(added)
}

/// Execute the info command
pub async fn execute_info(args: &super::InfoArgs) -> Result<()> {
    use crate::parsers::deb::DebParser;

    let parser = DebParser::new(&args.package)?;
    let metadata = parser.parse()?;

    match args.format {
        super::InfoFormat::Pretty => {
            println!("Package Information");
            println!("═══════════════════════════════════════");
            println!("Name:        {}", metadata.name);
            println!("Version:     {}", metadata.full_version());
            println!("Architecture: {}", metadata.arch);
            println!("Description: {}", metadata.description);
            
            if let Some(ref url) = metadata.url {
                println!("URL:         {}", url);
            }
            
            println!("License:     {}", metadata.license.to_pkgbuild());
            println!("Size:        {} bytes", metadata.installed_size);
            
            if let Some(ref maintainer) = metadata.maintainer {
                println!("Maintainer:  {}", maintainer);
            }

            if args.extended {
                println!("\nDependencies:");
                for dep in metadata.get_deps(crate::models::DependencyType::Depends) {
                    println!("  - {}", dep);
                }

                println!("\nFiles: {} total", metadata.files.len());
                for file in metadata.files.iter().take(10) {
                    println!("  {}", file.display());
                }
                if metadata.files.len() > 10 {
                    println!("  ... and {} more", metadata.files.len() - 10);
                }
            }
        }
        super::InfoFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&metadata)?);
        }
        super::InfoFormat::Toml => {
            println!("{}", toml::to_string_pretty(&metadata).map_err(|e| crate::error::RexebError::Other(e.to_string()))?);
        }
    }

    Ok(())
}

/// Execute the search command
pub async fn execute_search(args: &super::SearchArgs) -> Result<()> {
    use crate::resolver::database::PackageDatabase;
    use console::style;

    let db = PackageDatabase::new()?;
    
    let search_arch = args.arch || (!args.arch && !args.aur);
    let search_aur = args.aur || (!args.arch && !args.aur);

    let mut results = Vec::new();

    if search_arch {
        let arch_results = db.search_arch(&args.query, args.fuzzy, args.limit).await?;
        results.extend(arch_results.into_iter().map(|r| ("arch", r)));
    }

    if search_aur {
        let aur_results = db.search_aur(&args.query, args.fuzzy, args.limit).await?;
        results.extend(aur_results.into_iter().map(|r| ("aur", r)));
    }

    if results.is_empty() {
        println!("No packages found matching '{}'", args.query);
    } else {
        println!("Search Results for '{}'\n", style(&args.query).cyan());
        
        for (source, result) in results.iter().take(args.limit) {
            let source_badge = match *source {
                "arch" => style("[arch]").green(),
                "aur" => style("[aur]").yellow(),
                _ => style("[?]").dim(),
            };
            
            println!("{} {} - {}", source_badge, style(&result.name).bold(), result.description);
        }
    }

    Ok(())
}

/// Execute the analyze command
pub async fn execute_analyze(args: &super::AnalyzeArgs) -> Result<()> {
    use crate::parsers::deb::DebParser;
    use crate::analyzer::PackageAnalyzer;
    use console::style;

    let parser = DebParser::new(&args.input)?;
    let metadata = parser.parse()?;

    let analyzer = PackageAnalyzer::new(&metadata, parser.extract_dir())?;
    let report = analyzer.analyze(args.conflicts, args.verify)?;

    match args.format {
        super::InfoFormat::Pretty => {
            println!("{}", style("Package Analysis Report").bold().underlined());
            println!();
            
            // Summary
            println!("{}", style("Summary").bold());
            println!("  Package: {} {}", metadata.name, metadata.full_version());
            println!("  Architecture: {}", metadata.arch);
            println!("  Files: {}", metadata.files.len());
            println!("  Installed Size: {} KB", metadata.installed_size / 1024);
            println!();

            // Warnings
            if !report.warnings.is_empty() {
                println!("{}", style("⚠ Warnings").yellow().bold());
                for warning in &report.warnings {
                    println!("  • {}", warning);
                }
                println!();
            }

            // Errors
            if !report.errors.is_empty() {
                println!("{}", style("✗ Errors").red().bold());
                for error in &report.errors {
                    println!("  • {}", error);
                }
                println!();
            }

            // Dependency Analysis
            println!("{}", style("Dependency Analysis").bold());
            println!("  Total: {}", report.dependency_count);
            println!("  Mapped: {} ({:.1}%)", 
                report.mapped_count, 
                (report.mapped_count as f32 / report.dependency_count.max(1) as f32) * 100.0
            );
            println!("  Unmapped: {}", report.unmapped_deps.len());
            
            if !report.unmapped_deps.is_empty() {
                println!("\n  Unmapped dependencies:");
                for dep in &report.unmapped_deps {
                    println!("    - {}", dep);
                }
            }

            // Conflicts
            if args.conflicts && !report.conflicts.is_empty() {
                println!("\n{}", style("! Conflicts").red().bold());
                for conflict in &report.conflicts {
                    println!("  • {}", conflict);
                }
            }

            // File analysis
            if args.verify {
                println!("\n{}", style("File Verification").bold());
                println!("  Verified: {}", report.verified_files);
                println!("  Failed: {}", report.failed_files);
            }
        }
        super::InfoFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        super::InfoFormat::Toml => {
            println!("{}", toml::to_string_pretty(&report).map_err(|e| crate::error::RexebError::Other(e.to_string()))?);
        }
    }

    Ok(())
}

/// Execute the install command
pub async fn execute_install(args: &super::InstallArgs) -> Result<()> {
    use std::process::Command;
    use tempfile::TempDir;

    // Convert packages first
    let temp_dir = TempDir::new()?;
    let convert_args = super::ConvertArgs {
        input: args.input.clone(),
        output: Some(temp_dir.path().to_path_buf()),
        skip_deps: false,
        force: false,
        pkgbuild: false,
        yes: args.yes,
        pseudo64: false,
        keep_temp: false,
        sandbox: args.sandbox,
        name: None,
        version_override: None,
        release: None,
        format: super::OutputFormat::PkgTarZst,
    };

    execute_convert(&convert_args).await?;

    // Find converted packages
    let packages: Vec<_> = std::fs::read_dir(temp_dir.path())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "zst"))
        .map(|e| e.path())
        .collect();

    if packages.is_empty() {
        return Err(crate::error::RexebError::PackageBuild("No packages were created".into()));
    }

    // Build pacman command
    let mut cmd = Command::new("sudo");
    cmd.arg("pacman").arg("-U");

    if args.yes {
        cmd.arg("--noconfirm");
    }
    if args.asdeps {
        cmd.arg("--asdeps");
    }
    if args.asexplicit {
        cmd.arg("--asexplicit");
    }

    for arg in &args.pacman_args {
        cmd.arg(arg);
    }

    for pkg in &packages {
        cmd.arg(pkg);
    }

    // Execute pacman
    let status = cmd.status()?;

    if !status.success() {
        return Err(crate::error::RexebError::Other(
            format!("pacman exited with status: {}", status)
        ));
    }

    Ok(())
}

/// Execute the config command
pub async fn execute_config(args: &super::ConfigArgs) -> Result<()> {
    use crate::config::Config;

    match &args.command {
        super::ConfigCommands::Show => {
            let config = Config::load()?;
            println!("{}", toml::to_string_pretty(&config).map_err(|e| crate::error::RexebError::Other(e.to_string()))?);
        }
        super::ConfigCommands::Edit => {
            let config_path = Config::config_path()?;
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
            std::process::Command::new(editor)
                .arg(&config_path)
                .status()?;
        }
        super::ConfigCommands::Reset => {
            Config::reset()?;
            println!("Configuration reset to defaults");
        }
        super::ConfigCommands::Set { key, value } => {
            let mut config = Config::load()?;
            config.set(key, value)?;
            config.save()?;
            println!("Set {} = {}", key, value);
        }
        super::ConfigCommands::Get { key } => {
            let config = Config::load()?;
            if let Some(value) = config.get(key) {
                println!("{}", value);
            } else {
                println!("Key '{}' not found", key);
            }
        }
        super::ConfigCommands::Init { force } => {
            Config::init(*force)?;
            println!("Configuration initialized");
        }
    }

    Ok(())
}

/// Execute the clean command
pub async fn execute_clean(args: &super::CleanArgs) -> Result<()> {
    use crate::config::Config;
    use rayon::prelude::*;

    let config = Config::load()?;
    let clean_all = args.all || (!args.cache && !args.temp);

    let mut candidates = Vec::new();
    if clean_all || args.cache {
        candidates.push(config.cache_dir());
    }
    if clean_all || args.temp {
        candidates.push(std::env::temp_dir().join("rexeb"));
    }

    // Use rayon to filter candidates to existing directories in parallel
    let existing: Vec<_> = candidates
        .par_iter()
        .filter(|p| p.exists())
        .cloned()
        .collect();

    if args.dry_run {
        existing.par_iter().for_each(|p| {
            println!("Would remove: {}", p.display());
        });
    } else {
        // Remove directories in parallel and collect successfully cleaned ones
        let cleaned: Vec<_> = existing
            .par_iter()
            .filter_map(|p| match std::fs::remove_dir_all(p) {
                Ok(()) => Some(p.clone()),
                Err(e) => {
                    eprintln!("Failed to remove {}: {}", p.display(), e);
                    None
                }
            })
            .collect();

        if cleaned.is_empty() {
            println!("Nothing to clean");
        } else {
            println!("Cleaned {} directories", cleaned.len());
        }
    }

    Ok(())
}

/// Execute the map command
pub async fn execute_map(args: &super::MapArgs) -> Result<()> {
    use crate::resolver::database::PackageDatabase;
    use console::style;

    match &args.command {
        super::MapCommands::Add { debian, arch, confidence } => {
            let mut db = PackageDatabase::new()?;
            db.add_mapping(debian, arch, *confidence);
            db.save()?;
            println!("{} {} -> {} (confidence: {:.2})", style("Added:").green(), debian, arch, confidence);
        }
        super::MapCommands::Remove { debian } => {
            let _db = PackageDatabase::new()?;
            // Reload and remove: add_mapping then save with only that key removed would re-add; do direct remove
            // We need access to mappings - use save after manual removal via database file
            let db_dir = crate::config::Config::load().unwrap_or_default().data_dir().join("db");
            let mappings_path = if db_dir.join("mappings.json").exists() {
                db_dir.join("mappings.json")
            } else {
                dirs::data_dir().unwrap_or_default().join("rexeb").join("db").join("mappings.json")
            };
            // Try both locations - fallback: just try to update in-memory and persist
            if mappings_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&mappings_path) {
                    if let Ok(mut raw) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(inner) = raw.get_mut("mappings") {
                            if let Some(obj) = inner.as_object_mut() {
                                if obj.remove(debian).is_some() {
                                    std::fs::write(&mappings_path, serde_json::to_string_pretty(&raw)?)?;
                                    println!("{} {}", style("Removed:").yellow(), debian);
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
            // In-memory fallback
            let mappings_content = _db.save();
            let _ = mappings_content;
            println!("{} {} (in-memory; next load will not contain it if you export excluding it)", style("Note:").yellow(), debian);
        }
        super::MapCommands::List { json } => {
            let db = PackageDatabase::new()?;
            if *json {
                let count = db.get_arch_package_names().len();
                println!("{{\"arch_packages_known\": {}}}", count);
                println!("(Use `rexeb map export <file>` for full mappings dump)");
            }
            let count = db.get_arch_package_names().len();
            println!("Arch packages known: {}", count);
            println!("Mappings stored in: ~/.local/share/rexeb/db/mappings.json and db/mappings.json");
            println!("(Use `rexeb map export <file>` to dump current effective mappings)");
        }
        super::MapCommands::Import { file } => {
            let content = std::fs::read_to_string(file)?;
            let imported: std::collections::HashMap<String, crate::resolver::database::PackageMapping> = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(inner) = v.get("mappings") {
                    serde_json::from_value(inner.clone())?
                } else {
                    serde_json::from_value(v)?
                }
            } else {
                serde_json::from_str(&content)?
            };
            let mut db = PackageDatabase::new()?;
            let mut added = 0;
            for (k, m) in imported {
                db.add_mapping(&k, &m.arch_name, m.confidence);
                added += 1;
            }
            db.save()?;
            println!("Imported {} mappings", added);
        }
        super::MapCommands::Export { file } => {
            let db = PackageDatabase::new()?;
            // Export effective mappings from bundled + user overlay
            // We serialize by reading current db state via a temp round-trip
            let export: std::collections::HashMap<String, crate::resolver::database::PackageMapping> = {
                // Build from the bundled JSON + user overlay for a fair snapshot
                let mut all = std::collections::HashMap::new();
                for path in [std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("db/mappings.json")] {
                    if path.exists() {
                        if let Ok(c) = std::fs::read_to_string(&path) {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&c) {
                                if let Some(inner) = v.get("mappings") {
                                    if let Ok(m) = serde_json::from_value::<std::collections::HashMap<String, crate::resolver::database::PackageMapping>>(inner.clone()) {
                                        all.extend(m);
                                    }
                                }
                            }
                        }
                    }
                }
                // Overlay user file
                let user_path = crate::config::Config::load().unwrap_or_default().data_dir().join("db").join("mappings.json");
                for p in [dirs::data_dir().unwrap_or_default().join("rexeb").join("db").join("mappings.json"), user_path] {
                    if p.exists() {
                        if let Ok(c) = std::fs::read_to_string(&p) {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&c) {
                                if let Some(inner) = v.get("mappings") {
                                    if let Ok(m) = serde_json::from_value::<std::collections::HashMap<String, crate::resolver::database::PackageMapping>>(inner.clone()) {
                                        all.extend(m);
                                    }
                                } else if let Ok(m) = serde_json::from_value::<std::collections::HashMap<String, crate::resolver::database::PackageMapping>>(v) {
                                    all.extend(m);
                                }
                            }
                        }
                    }
                }
                all
            };
            let wrapped = serde_json::json!({"version": 1, "count": export.len(), "mappings": export});
            std::fs::write(file, serde_json::to_string_pretty(&wrapped)?)?;
            println!("Exported {} mappings to {}", export.len(), file.display());
            let _ = db;
        }
    }
    Ok(())
}

/// Execute the check-aur command
pub async fn execute_check_aur(args: &super::CheckAurArgs) -> Result<()> {
    use crate::resolver::aur::AurClient;
    use console::style;

    if args.installed {
        let installed = crate::watermark::list_installed()?;
        if installed.is_empty() {
            println!("No rexeb-installed packages found");
            return Ok(());
        }
        let client = AurClient::new();
        for pkg in &installed {
            let aur = client.info(&[&pkg.name]).await.unwrap_or_default();
            if let Some(aur_pkg) = aur.first() {
                let installed_ver = &pkg.version;
                let latest = aur_pkg.version.as_str();
                if installed_ver != latest {
                    println!("{} {} -> {} {} (installed: {})", style(&pkg.name).cyan(), style("update available:").yellow(), style(latest).green(), style(format!("(AUR: {} votes)", aur_pkg.num_votes)).dim(), installed_ver);
                } else {
                    println!("{} {} (latest: {})", style(&pkg.name).green(), style("up to date").dim(), latest);
                }
            } else {
                println!("{} {}", style(&pkg.name).dim(), style("not on AUR").dim());
            }
        }
        return Ok(());
    }

    let query = match &args.package {
        Some(q) => q.clone(),
        None => {
            eprintln!("Usage: rexeb check-aur <package.deb | package-name> [--installed]");
            return Ok(());
        }
    };

    // If query is a .deb file, parse its name
    let pkg_name = if std::path::Path::new(&query).extension().and_then(|e| e.to_str()) == Some("deb") {
        let p = std::path::Path::new(&query);
        if p.exists() {
            let parser = crate::parsers::deb::DebParser::new(p)?;
            let meta = parser.parse()?;
            meta.effective_name().to_string()
        } else {
            query.clone()
        }
    } else {
        query.clone()
    };

    println!("Checking AUR for '{}'...", style(&pkg_name).cyan());
    let client = AurClient::new();
    let results = client.info(&[&pkg_name]).await.unwrap_or_default();
    if results.is_empty() {
        let search = client.search(&pkg_name).await.unwrap_or_default();
        if search.is_empty() {
            println!("No AUR package found for '{}'", pkg_name);
        } else {
            println!("No exact match, but similar AUR packages:");
            for pkg in search.iter().take(5) {
                println!("  {} {} - {}", style(&pkg.name).green(), pkg.version, pkg.description.as_deref().unwrap_or(""));
            }
        }
    } else {
        for pkg in results {
            let outdated = pkg.out_of_date.map(|_| style(" (out of date)").red().to_string()).unwrap_or_default();
            println!("{} {}{} - {} (votes: {}, popularity: {:.2})", style(&pkg.name).green().bold(), pkg.version, outdated, pkg.description.as_deref().unwrap_or(""), pkg.num_votes, pkg.popularity);
            println!("  URL: {}", pkg.url.as_deref().unwrap_or("https://aur.archlinux.org"));
            println!("  Install: paru -S {}  or  yay -S {}", pkg.name, pkg.name);
        }
    }
    Ok(())
}

/// Execute the aur-push command
pub async fn execute_aur_push(args: &super::AurPushArgs) -> Result<()> {
    use console::style;

    let input = &args.input;
    let pkgbuild_dir: std::path::PathBuf;

    // Determine PKGBUILD location
    if input.is_dir() {
        let pb = input.join("PKGBUILD");
        if !pb.exists() {
            return Err(crate::error::RexebError::Validation(format!("No PKGBUILD in {}", input.display())));
        }
        pkgbuild_dir = input.clone();
    } else if input.extension().and_then(|e| e.to_str()) == Some("deb") {
        // Convert to PKGBUILD first
        let temp = tempfile::TempDir::new()?;
        let parser = crate::parsers::deb::DebParser::new(input)?;
        let mut meta = parser.parse()?;
        if let Some(ref pb) = args.pkgbase {
            meta.arch_name = Some(pb.clone());
        }
        meta.normalize_version();
        let resolver = crate::resolver::DependencyResolver::new()?;
        resolver.resolve(&mut meta).await?;
        let pb_content = meta.to_pkgbuild();
        std::fs::write(temp.path().join("PKGBUILD"), &pb_content)?;
        // Write SRCINFO helper
        let srcinfo = generate_srcinfo(&meta);
        std::fs::write(temp.path().join(".SRCINFO"), &srcinfo)?;
        println!("Generated PKGBUILD in {}", temp.path().display());
        pkgbuild_dir = temp.path().to_path_buf();
        if args.dry_run {
            println!("\n--- PKGBUILD ---\n{}\n--- .SRCINFO ---\n{}", pb_content, srcinfo);
            println!("\n{}: not pushing (dry run). To push:", style("Dry run").yellow());
            println!("  cd {} && git init && git add PKGBUILD .SRCINFO && git commit -m 'Initial import'", pkgbuild_dir.display());
            if let Some(pb) = args.pkgbase.as_deref().or(Some(meta.effective_name())) {
                println!("  git remote add aur ssh://aur@aur.archlinux.org/{}.git", pb);
                println!("  git push aur master");
            }
            return Ok(());
        }
        // Keep temp alive for push - leak it for now
        let _ = temp.keep();
        // Re-resolve path after keep
        if args.dry_run {
            return Ok(());
        }
    } else {
        return Err(crate::error::RexebError::Validation(format!("Input must be a .deb or directory with PKGBUILD: {}", input.display())));
    }

    if args.dry_run {
        let content = std::fs::read_to_string(pkgbuild_dir.join("PKGBUILD"))?;
        println!("\n--- PKGBUILD ({} ) ---\n{}", pkgbuild_dir.display(), content);
        let srcinfo_path = pkgbuild_dir.join(".SRCINFO");
        if srcinfo_path.exists() {
            println!("\n--- .SRCINFO ---\n{}", std::fs::read_to_string(&srcinfo_path)?);
        }
        println!("\n{}: would push to AUR as '{}'", style("Dry run").yellow(), args.pkgbase.as_deref().unwrap_or("from PKGBUILD"));
        return Ok(());
    }

    // Check if package exists on AUR
    let pkgbase = args.pkgbase.clone().unwrap_or_else(|| {
        // Parse pkgbase from PKGBUILD
        std::fs::read_to_string(pkgbuild_dir.join("PKGBUILD"))
            .ok()
            .and_then(|c| {
                for line in c.lines() {
                    if let Some(v) = line.strip_prefix("pkgbase=") {
                        return Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
                    }
                    if let Some(v) = line.strip_prefix("pkgname=") {
                        return Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
                    }
                }
                None
            })
            .unwrap_or_else(|| "rexeb-pkg".to_string())
    });

    if !args.force {
        let client = crate::resolver::aur::AurClient::new();
        if let Ok(existing) = client.info(&[&pkgbase]).await {
            if !existing.is_empty() {
                return Err(crate::error::RexebError::Validation(format!(
                    "Package '{}' already exists on AUR (version: {}). Use --force to overwrite or choose a different --pkgbase",
                    pkgbase, existing[0].version
                )));
            }
        }
    }

    // Init git if needed
    let git_dir = pkgbuild_dir.join(".git");
    if !git_dir.exists() {
        let s = std::process::Command::new("git").arg("init").current_dir(&pkgbuild_dir).status()?;
        if !s.success() {
            return Err(crate::error::RexebError::Other("git init failed".into()));
        }
    }

    // Generate .SRCINFO if missing
    if !pkgbuild_dir.join(".SRCINFO").exists() {
        if let Ok(content) = std::fs::read_to_string(pkgbuild_dir.join("PKGBUILD")) {
            let meta = crate::models::PackageMetadata::new(&pkgbase, "0");
            let srcinfo = generate_srcinfo(&meta);
            std::fs::write(pkgbuild_dir.join(".SRCINFO"), srcinfo)?;
            let _ = content;
            let _ = meta;
        }
    }

    std::process::Command::new("git").args(["add", "PKGBUILD", ".SRCINFO"]).current_dir(&pkgbuild_dir).status()?;
    let status = std::process::Command::new("git").args(["-c", "user.name=rexeb", "-c", "user.email=rexeb@local", "commit", "-m", &format!("Upstream import {}", pkgbase)]).current_dir(&pkgbuild_dir).status()?;
    if !status.success() {
        // May be "nothing to commit" - not fatal
        tracing::warn!("git commit returned non-zero (possibly nothing to commit)");
    }

    // Add AUR remote if not present
    let remote_url = format!("ssh://aur@aur.archlinux.org/{}.git", pkgbase);
    let has_remote = std::process::Command::new("git").args(["remote", "get-url", "aur"]).current_dir(&pkgbuild_dir).output().map(|o| o.status.success()).unwrap_or(false);
    if !has_remote {
        std::process::Command::new("git").args(["remote", "add", "aur", &remote_url]).current_dir(&pkgbuild_dir).status()?;
    }

    println!("Pushing to {} ...", remote_url);
    let push = std::process::Command::new("git").args(["push", "aur", "master"]).current_dir(&pkgbuild_dir).status()?;
    if !push.success() {
        // Try main branch
        let push2 = std::process::Command::new("git").args(["push", "aur", "HEAD:master"]).current_dir(&pkgbuild_dir).status()?;
        if !push2.success() {
            return Err(crate::error::RexebError::Other(format!("git push to AUR failed for {}", pkgbase)));
        }
    }

    println!("{} Published '{}' to AUR. Users can now run: {} or {}", style("Success:").green(), pkgbase, format!("paru -S {}", pkgbase), format!("yay -S {}", pkgbase));
    Ok(())
}

fn generate_srcinfo(meta: &crate::models::PackageMetadata) -> String {
    let mut s = String::new();
    s.push_str(&format!("pkgbase = {}\n", meta.effective_name()));
    s.push_str(&format!("\tpkgdesc = {}\n", meta.description));
    s.push_str(&format!("\tpkgver = {}\n", meta.version));
    s.push_str(&format!("\tpkgrel = {}\n", meta.release));
    s.push_str(&format!("\turl = {}\n", meta.url.as_deref().unwrap_or("https://github.com/onionorbit/rexeb")));
    s.push_str(&format!("\tarch = {}\n", meta.arch.to_arch_name()));
    s.push_str(&format!("\tlicense = {}\n", meta.license.to_pkgbuild()));
    for dep in meta.get_deps(crate::models::DependencyType::Depends) {
        s.push_str(&format!("\tdepends = {}\n", dep.to_arch_string()));
    }
    s.push_str(&format!("\n{} = {}\n", format!("pkgname = {}", meta.effective_name()), ""));
    s
}

/// Execute list-installed command
pub async fn execute_list_installed(args: &super::ListInstalledArgs) -> Result<()> {
    use console::style;
    let pkgs = crate::watermark::list_installed()?;
    if pkgs.is_empty() {
        println!("No packages installed by rexeb were found");
        println!("(watermark: x-rexeb in PKGINFO or /usr/share/doc/<pkg>/.rexeb.json)");
        return Ok(());
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&pkgs)?);
    } else {
        println!("{} ({} packages)\n", style("Packages installed by rexeb").cyan().bold(), pkgs.len());
        for p in &pkgs {
            let det = match p.detection {
                crate::watermark::DetectionMethod::PkgInfo => "watermark",
                crate::watermark::DetectionMethod::Sentinel => "sentinel",
                crate::watermark::DetectionMethod::DocSentinel => "doc-sentinel",
            };
            println!("  {} {} {} [{}]", style(&p.name).green(), p.version, style(format!("({})", det)).dim(), p.db_path.display());
            if let Some(wm) = &p.watermark {
                println!("    rexeb v{}, source: {}, at {}", wm.rexeb_version, wm.source_deb, wm.converted_at);
            }
        }
    }
    Ok(())
}

/// Execute the manage command
pub async fn execute_manage(args: &super::ManageArgs) -> Result<()> {
    use console::style;

    if let Some(new_name) = &args.rename {
        crate::watermark::rename_installed(&args.package, new_name)?;
        println!("{} Alias '{}' -> '{}' saved. Reconvert with --name to actually rename the package file.", style("Note:").yellow(), args.package, new_name);
    }

    if args.fix_icon {
        let fixes = crate::watermark::fix_icons(&args.package)?;
        if fixes.is_empty() {
            println!("No icon fixes needed for '{}'", args.package);
        } else {
            for f in &fixes {
                println!("  {}", f);
            }
            println!("Fixed {} icon entries. You may need to reinstall with: rexeb convert --name <pkg> <deb> && sudo pacman -U <pkg>", fixes.len());
        }
    }

    if args.rename.is_none() && !args.fix_icon {
        println!("Nothing to do. Use --rename <NEW> or --fix-icon");
    }

    Ok(())
}

/// Execute self-update (check GitHub releases)
pub async fn execute_self_update(args: &super::SelfUpdateArgs) -> Result<()> {
    use console::style;

    let current = crate::VERSION;
    println!("{} v{} — checking for updates...", style("rexeb").cyan().bold(), current);

    let api_url = "https://api.github.com/repos/onionorbit/rexeb/releases/latest";
    let cargo_url = "https://raw.githubusercontent.com/onionorbit/rexeb/main/Cargo.toml";

    let client = reqwest::Client::builder()
        .user_agent(format!("rexeb/{}", current))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| crate::error::RexebError::Network(e.to_string()))?;

    // Try GitHub Releases API first
    let mut latest_version: Option<String> = None;
    let mut latest_tag: Option<String> = None;

    if let Ok(resp) = client
        .get(api_url)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                if let Some(tag) = val.get("tag_name").and_then(|v| v.as_str()) {
                    latest_tag = Some(tag.to_string());
                    latest_version = Some(tag.trim_start_matches('v').to_string());
                }
            }
        } else if resp.status().as_u16() == 403 || resp.status().as_u16() == 404 {
            tracing::debug!("GitHub releases API returned {} — falling back to Cargo.toml", resp.status());
        }
    }

    // Fallback: parse Cargo.toml version
    if latest_version.is_none() {
        if let Ok(resp) = client.get(cargo_url).send().await {
            if resp.status().is_success() {
                if let Ok(text) = resp.text().await {
                    for line in text.lines() {
                        let t = line.trim();
                        if t.starts_with("version") && t.contains('=') {
                            if let Some(v) = t.split('"').nth(1) {
                                latest_version = Some(v.to_string());
                                latest_tag = Some(format!("v{}", v));
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    let latest = match latest_version {
        Some(v) => v,
        None => {
            println!("{} Could not determine latest version (network unavailable). Check https://github.com/onionorbit/rexeb/releases", style("Note:").yellow());
            return Ok(());
        }
    };

    if latest == current {
        println!("{} You are on the latest version ({})", style("Up to date:").green(), current);
        return Ok(());
    }

    // Simple semver compare: split by '.' and compare numerically
    let is_newer = is_version_newer(current, &latest);
    if !is_newer && !args.force {
        println!("Current ({}) is newer or equal to latest ({}). Use --force to show update anyway.", current, latest);
        return Ok(());
    }

    println!("\n{} Update available: {} -> {}", style("Update:").yellow().bold(), style(current).dim(), style(&latest).green().bold());
    if let Some(tag) = &latest_tag {
        println!("  Release: https://github.com/onionorbit/rexeb/releases/tag/{}", tag);
    }
    println!("\n  Update via:");
    println!("    paru -S rexeb        # if installed via AUR");
    println!("    cargo install --git https://github.com/onionorbit/rexeb");
    println!("    or download from: https://github.com/onionorbit/rexeb/releases/latest");

    if args.check_only {
        return Ok(());
    }

    println!("\n{} Run one of the update commands above to upgrade.", style("Next steps:").cyan());
    Ok(())
}

fn is_version_newer(current: &str, latest: &str) -> bool {
    // Strip pre-release suffixes (-alpha, -beta)
    let strip = |s: &str| -> String { s.split('-').next().unwrap_or(s).to_string() };
    let cur = strip(current);
    let lat = strip(latest);
    let parse = |s: &str| s.split('.').filter_map(|p| p.parse::<u64>().ok()).collect::<Vec<_>>();
    let c: Vec<u64> = parse(&cur);
    let l: Vec<u64> = parse(&lat);
    for (a, b) in c.iter().zip(l.iter()) {
        if b > a { return true; }
        if a > b { return false; }
    }
    l.len() > c.len()
}
