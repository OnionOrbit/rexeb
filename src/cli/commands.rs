//! Command execution handlers

use std::path::Path;

use crate::error::Result;

/// Execute the convert command
pub async fn execute_convert(
    args: &super::ConvertArgs,
    quiet: bool,
    jobs: Option<usize>,
) -> Result<()> {
    use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

    let config = crate::config::Config::load().unwrap_or_default();
    let output_dir = args
        .output
        .clone()
        .or(config.general.output_dir.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
    std::fs::create_dir_all(&output_dir)?;

    // No inputs: interactive picker on a TTY, hard error otherwise
    let promptable = super::interactive::is_promptable(args.yes, quiet);
    let inputs: Vec<std::path::PathBuf> = if args.input.is_empty() {
        if promptable {
            super::interactive::pick_input_files()?
        } else {
            return Err(crate::error::RexebError::Validation(
                "No input files given".into(),
            ));
        }
    } else {
        args.input.clone()
    };

    let multi = MultiProgress::new();
    if quiet || promptable {
        // Prompts and progress bars fight over the terminal; when prompts
        // may appear, conversions print plain status lines instead.
        multi.set_draw_target(ProgressDrawTarget::hidden());
    }
    let style = ProgressStyle::with_template(
        "{prefix:.bold.dim} [{bar:40.cyan/blue}] {pos}/{len} {msg}"
    )
    .unwrap()
    .progress_chars("█▓▒░ ");

    // Bound concurrency: each conversion does heavy blocking work
    // (extraction, hashing, zstd-19), so unbounded spawns would starve the
    // runtime and OOM small devices. Prompting conversions run strictly
    // serially so only one task ever owns stdin.
    let permits = if promptable {
        1
    } else {
        crate::effective_parallel_jobs(jobs.or(config.general.jobs))
    };
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(permits));

    // Process packages using tasks since we're async now
    let mut handles = Vec::new();

    for input_path in &inputs {
        let permit = sem
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| crate::error::RexebError::Other(e.to_string()))?;
        let input_path = input_path.clone();
        let output_dir = output_dir.clone();
        let args_clone = args.clone();

        let pb = multi.add(ProgressBar::new(100));
        pb.set_style(style.clone());
        pb.set_prefix(format!("{}", input_path.file_name().unwrap_or_default().to_string_lossy()));

        handles.push(tokio::spawn(async move {
            let _permit = permit;
            convert_single_package(&input_path, &output_dir, &args_clone, pb, quiet).await
        }));
    }

    // Wait for all tasks, collecting every failure instead of aborting on
    // the first one while siblings keep running detached.
    let total = handles.len();
    let mut failed = 0usize;
    for handle in handles {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                eprintln!("Error: {}", e);
                failed += 1;
            }
            Err(e) => {
                eprintln!("Task failed: {}", e);
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(crate::error::RexebError::Other(format!(
            "{}/{} conversions failed",
            failed, total
        )));
    }

    Ok(())
}

/// Convert a single package
async fn convert_single_package(
    input: &Path,
    output_dir: &Path,
    args: &super::ConvertArgs,
    pb: indicatif::ProgressBar,
    quiet: bool,
) -> Result<()> {
    use crate::converter::PackageConverter;
    use crate::models::DependencyType;

    // CLI flags win; config file supplies the defaults
    let config = crate::config::Config::load().unwrap_or_default();
    let skip_deps = args.skip_deps || config.conversion.skip_deps;
    let keep_temp = args.keep_temp || config.conversion.keep_temp;
    let gen_pkgbuild = args.pkgbuild || config.conversion.generate_pkgbuild;
    let strip = config.conversion.strip_binaries;
    let format = args
        .format
        .or_else(|| super::OutputFormat::from_config_str(&config.conversion.default_format))
        .unwrap_or(super::OutputFormat::PkgTarZst);
    let promptable = super::interactive::is_promptable(args.yes, quiet);

    if promptable {
        // Progress bars are hidden in this mode; plain lines are the UI
        println!("Converting {} ...", input.display());
    }
    pb.set_message("Parsing package...");
    pb.set_position(10);

    // Parse the input package (auto-detected: .deb, .rpm, .AppImage)
    let parser = crate::parsers::detect_and_create(input)?;
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

    // Experimental 32-bit compat mode (debtap-style --pseudo-64-bit)
    if args.pseudo64 {
        if metadata.arch == crate::models::Architecture::I686 {
            tracing::warn!("--pseudo64: treating i386 package as x86_64 (experimental)");
            metadata.arch = crate::models::Architecture::X86_64;
        } else {
            tracing::warn!(
                "--pseudo64 only affects 32-bit (i386) packages; ignoring for {}",
                metadata.arch
            );
        }
    }

    // Normalize version
    metadata.normalize_version();

    pb.set_position(40);

    // Resolve dependencies if not skipped (also prunes self-references)
    let resolver = crate::resolver::DependencyResolver::new()?;
    if !skip_deps {
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

    // Unmapped hard dependencies would silently vanish from the package —
    // review them with the user when possible, warn loudly otherwise.
    if !skip_deps {
        let unmapped: Vec<String> = [DependencyType::Depends, DependencyType::PreDepends]
            .iter()
            .flat_map(|t| metadata.get_deps(*t))
            .filter(|d| !d.is_mapped())
            .map(|d| d.debian_name.clone())
            .collect();
        let soft_unmapped = [DependencyType::Recommends, DependencyType::Suggests]
            .iter()
            .flat_map(|t| metadata.get_deps(*t))
            .filter(|d| !d.is_mapped())
            .count();
        if !unmapped.is_empty() {
            if promptable {
                super::interactive::review_unmapped_dependencies(&mut metadata).await?;
            } else {
                tracing::warn!(
                    "{} unmapped dependencies will be dropped from {}: {}",
                    unmapped.len(),
                    metadata.effective_name(),
                    unmapped.join(", ")
                );
            }
        }
        if soft_unmapped > 0 {
            tracing::warn!(
                "{} unmapped optional dependencies will be dropped from {}",
                soft_unmapped,
                metadata.effective_name()
            );
        }
    }

    pb.set_position(60);
    pb.set_message("Building package...");

    if args.dry_run {
        print_dry_run_plan(input, output_dir, &metadata, format, gen_pkgbuild, skip_deps);
        pb.set_position(100);
        pb.finish_with_message("Dry run complete");
        return Ok(());
    }

    if args.interactive && promptable {
        let file_name = PackageConverter::package_file_name(&metadata, format);
        let question = if gen_pkgbuild {
            format!(
                "Generate PKGBUILD in {}/{}?",
                output_dir.display(),
                metadata.effective_name()
            )
        } else {
            format!("Build {}?", output_dir.join(&file_name).display())
        };
        if !super::interactive::confirm_proceed(&question)? {
            return Err(crate::error::RexebError::Validation(
                "Conversion aborted by user".into(),
            ));
        }
    }

    // Create output package
    if gen_pkgbuild {
        // Per-package directory: batch conversions would otherwise race on
        // a single shared PKGBUILD path
        let dir = output_dir.join(metadata.effective_name());
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("PKGBUILD"), metadata.to_pkgbuild())?;
        std::fs::write(dir.join(".SRCINFO"), generate_srcinfo(&metadata))?;
        pb.set_position(100);
        pb.finish_with_message(format!("Created {}/PKGBUILD", dir.display()));
        if promptable {
            println!("Created {}/PKGBUILD", dir.display());
        }
    } else {
        // Build binary package
        let source_name = input
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("package")
            .to_string();

        // Sandbox backend: bubblewrap isolates the build, nspawn stages
        // files through a container (auto prefers bubblewrap)
        let backend = args
            .sandbox_backend
            .unwrap_or(super::SandboxBackendArg::Auto);
        let isolated = if args.sandbox {
            match backend {
                super::SandboxBackendArg::Bwrap => {
                    if !crate::sandbox::bwrap_available() {
                        return Err(crate::error::RexebError::PackageBuild(
                            "bubblewrap (bwrap) not found — install bubblewrap or use --sandbox-backend nspawn".into(),
                        ));
                    }
                    true
                }
                super::SandboxBackendArg::Nspawn => false,
                super::SandboxBackendArg::Auto => {
                    let available = crate::sandbox::bwrap_available();
                    if !available {
                        tracing::warn!(
                            "bubblewrap not found — falling back to nspawn staging (not full isolation)"
                        );
                    }
                    available
                }
            }
        } else {
            false
        };

        // Computed before `metadata` moves into the isolated manifest
        let expected = output_dir.join(PackageConverter::package_file_name(&metadata, format));

        let output_path;
        if isolated {
            pb.set_message("Building isolated (bubblewrap)...");
            output_path = crate::sandbox::run_isolated_build(crate::sandbox::IsolatedBuild {
                metadata,
                host_data_dir: parser.extract_dir().to_path_buf(),
                output_path: expected,
                format,
                overwrite: args.force,
                strip_binaries: strip,
                source_file: Some(source_name),
                keep_temp,
            })?;
        } else {
            let mut converter = PackageConverter::new(metadata, parser.extract_dir())?
                .with_overwrite(args.force)
                .with_strip_binaries(strip)
                .with_source_file(source_name);

            // Optionally stage through an nspawn sandbox (guard kept alive
            // for the build)
            let mut _sandbox_guard: Option<tempfile::TempDir> = None;
            if args.sandbox {
                let guard = tempfile::Builder::new().prefix("rexeb-").tempdir()?;
                let root = guard.path().to_path_buf();
                converter = converter.with_sandbox(&root)?;
                _sandbox_guard = Some(guard);
                pb.set_message("Building in sandbox...");
            }

            output_path = converter.build(output_dir, format)?;
        }

        if args.sign || args.sign_key.is_some() {
            let sig = sign_package(&output_path, args.sign_key.as_deref())?;
            if promptable {
                println!("Signed {}", sig.display());
            } else {
                pb.println(format!("Signed {}", sig.display()));
            }
        }

        pb.set_position(100);
        pb.finish_with_message(format!("Created {}", output_path.display()));
        if promptable {
            println!("Created {}", output_path.display());
        }
    }

    if keep_temp {
        let kept = parser.persist();
        if promptable {
            println!("Kept temp dir: {}", kept.display());
        } else {
            pb.println(format!("Kept temp dir: {}", kept.display()));
        }
    }

    Ok(())
}

/// Print what a conversion would do, without writing anything
fn print_dry_run_plan(
    input: &Path,
    output_dir: &Path,
    metadata: &crate::models::PackageMetadata,
    format: super::OutputFormat,
    gen_pkgbuild: bool,
    skip_deps: bool,
) {
    use console::style;
    use crate::models::DependencyType;

    println!("{} {}", style("Dry run:").cyan().bold(), input.display());
    if gen_pkgbuild {
        println!(
            "  Output:   {}/{{PKGBUILD,.SRCINFO}}",
            output_dir.join(metadata.effective_name()).display()
        );
    } else {
        let file_name =
            crate::converter::PackageConverter::package_file_name(metadata, format);
        println!("  Output:   {}", output_dir.join(&file_name).display());
        println!("  Format:   {}", format.extension());
    }
    println!(
        "  Package:  {} {}-{} ({})",
        metadata.effective_name(),
        metadata.version,
        metadata.release,
        metadata.arch.to_arch_name()
    );
    println!("  Files:    {}", metadata.files.len());
    if skip_deps {
        println!("  Depends:  (resolution skipped)");
        return;
    }

    let show = |title: &str, types: &[DependencyType]| {
        let mut mapped = Vec::new();
        let mut unmapped = Vec::new();
        for t in types {
            for d in metadata.get_deps(*t) {
                if d.is_mapped() {
                    mapped.push(format!("{} (from {})", d.to_arch_string(), d.debian_name));
                } else {
                    unmapped.push(d.debian_name.clone());
                }
            }
        }
        if mapped.is_empty() && unmapped.is_empty() {
            return;
        }
        println!("  {}:", title);
        for m in mapped {
            println!("    {} {}", style("✓").green(), m);
        }
        for u in unmapped {
            println!(
                "    {} {} (UNMAPPED — will be dropped)",
                style("✗").red(),
                u
            );
        }
    };
    show(
        "Depends",
        &[DependencyType::Depends, DependencyType::PreDepends],
    );
    show(
        "Optdepends",
        &[DependencyType::Recommends, DependencyType::Suggests],
    );

    let provides = metadata.get_deps(DependencyType::Provides);
    if !provides.is_empty() {
        let list: Vec<String> = provides.iter().map(|d| d.to_arch_string()).collect();
        println!("  Provides: {}", list.join(", "));
    }
}

/// Detach-sign a built package with GPG, returning the `.sig` path
fn sign_package(path: &Path, key: Option<&str>) -> Result<std::path::PathBuf> {
    let have_gpg = std::process::Command::new("gpg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !have_gpg {
        return Err(crate::error::RexebError::PackageBuild(
            "gpg not found — cannot sign (install gnupg)".into(),
        ));
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "package".to_string());
    let sig_path = path.with_file_name(format!("{}.sig", file_name));

    let mut cmd = std::process::Command::new("gpg");
    cmd.arg("--detach-sign").arg("--yes");
    if let Some(k) = key {
        cmd.arg("--local-user").arg(k);
    }
    cmd.arg("--output").arg(&sig_path).arg(path);
    let output = cmd.output()?;
    if !output.status.success() {
        return Err(crate::error::RexebError::PackageBuild(format!(
            "gpg signing failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(sig_path)
}

/// Execute the completions command
pub async fn execute_completions(args: &super::CompletionsArgs) -> Result<()> {
    use clap::CommandFactory;
    let mut cmd = super::Cli::command();
    clap_complete::generate(
        args.shell.to_generator(),
        &mut cmd,
        "rexeb".to_string(),
        &mut std::io::stdout(),
    );
    Ok(())
}

/// Execute the manpage command
pub async fn execute_manpage(args: &super::ManpageArgs) -> Result<()> {
    use clap::CommandFactory;
    let cmd = super::Cli::command();
    let man = clap_mangen::Man::new(cmd);
    let mut buf: Vec<u8> = Vec::new();
    man.render(&mut buf)?;
    match &args.output {
        Some(path) => {
            std::fs::write(path, buf)?;
            println!("Wrote {}", path.display());
        }
        None => {
            use std::io::Write;
            std::io::stdout().write_all(&buf)?;
        }
    }
    Ok(())
}

/// Execute the hidden sandbox-build command (re-exec target for isolated builds)
pub fn execute_sandbox_build(args: &super::SandboxBuildArgs) -> Result<()> {
    use crate::converter::PackageConverter;

    let content = std::fs::read_to_string(&args.manifest)?;
    let manifest: crate::sandbox::SandboxManifest = serde_json::from_str(&content)?;
    std::fs::create_dir_all(&args.out_dir)?;
    let converter = PackageConverter::new(manifest.metadata, &manifest.data_dir)?
        .with_overwrite(manifest.overwrite)
        .with_strip_binaries(manifest.strip_binaries);
    let converter = match manifest.source_file {
        Some(name) => converter.with_source_file(name),
        None => converter,
    };
    let output = converter.build(&args.out_dir, manifest.format)?;
    // The parent collects the artifact by scanning the out dir; the printed
    // name is for logs and debugging.
    println!(
        "{}",
        output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    Ok(())
}

/// Execute the update command
pub async fn execute_update(args: &super::UpdateArgs, quiet: bool) -> Result<()> {
    use crate::resolver::database::PackageDatabase;
    use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

    let style = ProgressStyle::with_template("{spinner:.green} {msg}").unwrap();

    let pb = ProgressBar::new_spinner();
    pb.set_style(style);
    if quiet {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
    }

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
        let added = db.enlarge().await?;
        if !quiet {
            if added > 0 {
                println!("Enlarged: {} new mappings proposed from local DB scan", added);
            } else {
                println!("No new mappings found during enlarge scan");
            }
        }
    }

    pb.finish_with_message("Database updated successfully");
    Ok(())
}

/// Execute the info command
pub async fn execute_info(args: &super::InfoArgs) -> Result<()> {
    let parser = crate::parsers::detect_and_create(&args.package)?;
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
            println!("{}", to_toml_pretty(&metadata)?);
        }
    }

    Ok(())
}

/// Convert a JSON value to TOML
///
/// Metadata contains maps with non-string keys (enums, paths) which TOML
/// cannot serialize directly, so values go through a JSON round-trip first.
fn json_to_toml(value: &serde_json::Value) -> toml::Value {
    match value {
        serde_json::Value::Null => toml::Value::String("null".to_string()),
        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        serde_json::Value::Array(a) => toml::Value::Array(a.iter().map(json_to_toml).collect()),
        serde_json::Value::Object(o) => toml::Value::Table(
            o.iter().map(|(k, v)| (k.clone(), json_to_toml(v))).collect(),
        ),
    }
}

/// Serialize any value as pretty TOML via a JSON round-trip
fn to_toml_pretty<T: serde::Serialize>(value: &T) -> Result<String> {
    let json = serde_json::to_value(value)?;
    let toml_value = json_to_toml(&json);
    toml::to_string_pretty(&toml_value)
        .map_err(|e| crate::error::RexebError::Other(e.to_string()))
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
    use crate::analyzer::PackageAnalyzer;
    use console::style;

    let parser = crate::parsers::detect_and_create(&args.input)?;
    let mut metadata = parser.parse()?;
    metadata.normalize_version();

    // Resolve first: without this every dependency reports as unmapped and
    // the analysis is useless
    let resolver = crate::resolver::DependencyResolver::new()?;
    resolver.resolve(&mut metadata).await?;

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
            println!("{}", to_toml_pretty(&report)?);
        }
    }

    Ok(())
}

/// Execute the install command
pub async fn execute_install(args: &super::InstallArgs, quiet: bool) -> Result<()> {
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
        interactive: false,
        dry_run: false,
        sign: false,
        sign_key: None,
        pseudo64: false,
        keep_temp: false,
        sandbox: args.sandbox,
        sandbox_backend: None,
        name: None,
        version_override: None,
        release: None,
        format: None,
    };

    execute_convert(&convert_args, quiet, None).await?;

    // Find converted packages (any .pkg.tar.* compression)
    let packages: Vec<_> = std::fs::read_dir(temp_dir.path())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map_or(false, |n| n.contains(".pkg.tar."))
        })
        .collect();

    if packages.is_empty() {
        return Err(crate::error::RexebError::PackageBuild("No packages were created".into()));
    }

    // Build pacman command (no sudo when already root — containers often
    // lack sudo entirely)
    let is_root = Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false);
    let mut cmd = if is_root {
        Command::new("pacman")
    } else {
        let mut c = Command::new("sudo");
        c.arg("pacman");
        c
    };
    cmd.arg("-U");

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
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if !config_path.exists() {
                Config::default().save()?;
            }
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
            let status = std::process::Command::new(&editor)
                .arg(&config_path)
                .status()
                .map_err(|e| {
                    crate::error::RexebError::Other(format!(
                        "Could not launch editor '{}': {}",
                        editor, e
                    ))
                })?;
            if !status.success() {
                return Err(crate::error::RexebError::Other(format!(
                    "Editor exited with status: {}",
                    status
                )));
            }
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
        super::ConfigCommands::Init { force, interactive } => {
            if *interactive {
                if !super::interactive::is_promptable(false, false) {
                    return Err(crate::error::RexebError::Validation(
                        "config init --interactive needs a terminal".into(),
                    ));
                }
                let path = Config::config_path()?;
                if path.exists() && !*force {
                    return Err(crate::error::RexebError::Config(
                        "Configuration file already exists. Use --force to overwrite.".into(),
                    ));
                }
                let config = super::interactive::prompt_config_wizard()?;
                config.save()?;
                println!("Configuration initialized at {}", path.display());
            } else {
                Config::init(*force)?;
                println!("Configuration initialized");
            }
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
        // Temp dirs are created with a `rexeb-` prefix (see DebParser and
        // the sandbox/aur-push flows); collect leftovers from --keep-temp,
        // crashes, or older runs.
        if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_str()
                    .map_or(false, |n| n.starts_with("rexeb-"))
                {
                    candidates.push(entry.path());
                }
            }
        }
    }

    // Filter to paths that actually exist (in parallel for large temp dirs)
    let existing: Vec<_> = candidates
        .par_iter()
        .filter(|p| p.exists())
        .cloned()
        .collect();

    if args.dry_run {
        if existing.is_empty() {
            println!("Nothing to clean");
        } else {
            for p in &existing {
                println!("Would remove: {}", p.display());
            }
        }
    } else {
        // Remove in parallel (files and dirs) and report afterwards so
        // parallel println! calls cannot interleave mid-line
        let cleaned: Vec<_> = existing
            .par_iter()
            .filter_map(|p| {
                let removed = if p.is_dir() {
                    std::fs::remove_dir_all(p)
                } else {
                    std::fs::remove_file(p)
                };
                match removed {
                    Ok(()) => Some(p.clone()),
                    Err(e) => {
                        eprintln!("Failed to remove {}: {}", p.display(), e);
                        None
                    }
                }
            })
            .collect();

        if cleaned.is_empty() {
            println!("Nothing to clean");
        } else {
            println!("Cleaned {} paths", cleaned.len());
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
            let (debian, arch, confidence) =
                if debian.is_none() || arch.is_none() {
                    if !super::interactive::is_promptable(false, false) {
                        return Err(crate::error::RexebError::Validation(
                            "map add needs debian and arch names (no terminal for prompting)".into(),
                        ));
                    }
                    super::interactive::prompt_map_add(
                        debian.clone(),
                        arch.clone(),
                        *confidence,
                    )?
                } else {
                    (
                        debian.clone().unwrap_or_default(),
                        arch.clone().unwrap_or_default(),
                        *confidence,
                    )
                };
            if !confidence.is_finite() || confidence < 0.0 || confidence > 1.0 {
                return Err(crate::error::RexebError::Validation(
                    "confidence must be between 0.0 and 1.0".into(),
                ));
            }
            let mut db = PackageDatabase::new()?;
            db.add_mapping(&debian, &arch, confidence);
            db.save()?;
            println!("{} {} -> {} (confidence: {:.2})", style("Added:").green(), debian, arch, confidence);
        }
        super::MapCommands::Remove { debian } => {
            let mut db = PackageDatabase::new()?;
            if db.remove_mapping(debian) {
                db.save()?;
                println!("{} {}", style("Removed:").yellow(), debian);
            } else {
                println!("{} no mapping for '{}'", style("Note:").yellow(), debian);
            }
        }
        super::MapCommands::List { json } => {
            let db = PackageDatabase::new()?;
            let mappings = db.mappings();
            if *json {
                println!("{}", serde_json::to_string_pretty(mappings)?);
            } else {
                println!("{} effective mappings:", mappings.len());
                let mut keys: Vec<&String> = mappings.keys().collect();
                keys.sort();
                for key in keys.iter().take(50) {
                    let m = &mappings[*key];
                    println!("  {} -> {} ({:.2})", m.debian_name, m.arch_name, m.confidence);
                }
                if mappings.len() > 50 {
                    println!("  ... and {} more (use --json or `map export`)", mappings.len() - 50);
                }
            }
        }
        super::MapCommands::Import { file } => {
            use std::collections::HashMap;
            let content = std::fs::read_to_string(file)?;
            let value: serde_json::Value = serde_json::from_str(&content)?;
            let inner = value.get("mappings").cloned().unwrap_or(value);
            // Accept the full mapping objects as well as a simple
            // {"debian": "arch"} shape (confidence 1.0)
            let imported: HashMap<String, crate::resolver::database::PackageMapping> =
                serde_json::from_value(inner.clone()).or_else(|_| {
                    let simple: HashMap<String, String> = serde_json::from_value(inner)?;
                    Ok::<_, serde_json::Error>(
                        simple
                            .into_iter()
                            .map(|(k, v)| {
                                (
                                    k.clone(),
                                    crate::resolver::database::PackageMapping {
                                        debian_name: k,
                                        arch_name: v,
                                        confidence: 1.0,
                                        source: crate::resolver::database::MappingSource::User,
                                    },
                                )
                            })
                            .collect(),
                    )
                })?;
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
            if let Some(parent) = file.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            let wrapped = serde_json::json!({
                "version": 1,
                "count": db.mappings().len(),
                "mappings": db.mappings(),
            });
            std::fs::write(file, serde_json::to_string_pretty(&wrapped)?)?;
            println!("Exported {} mappings to {}", db.mappings().len(), file.display());
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
            let aur = match client.info(&[pkg.name.as_str()]).await {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("{} AUR lookup failed for '{}': {}", style("Warning:").yellow(), pkg.name, e);
                    continue;
                }
            };
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
            eprintln!("Usage: rexeb check-aur <package.deb | package.rpm | package.AppImage | package-name> [--installed]");
            return Ok(());
        }
    };

    // If the query is a package file, resolve its package name (AppImages
    // use the cheap filename heuristic — no need to extract gigabytes)
    let p = std::path::Path::new(&query);
    let known_ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "deb" | "rpm" | "appimage"
            )
        })
        .unwrap_or(false);
    let pkg_name = if p.exists() && crate::parsers::appimage::is_appimage(p) {
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_else(|| query.as_str());
        crate::parsers::appimage::split_appimage_filename(stem).0
    } else if p.exists() {
        match crate::parsers::detect_and_create(p) {
            Ok(parser) => parser.parse()?.effective_name().to_string(),
            Err(_) if !known_ext => query.clone(),
            Err(e) => return Err(e),
        }
    } else {
        query.clone()
    };

    println!("Checking AUR for '{}'...", style(&pkg_name).cyan());
    let client = AurClient::new();
    let results = client.info(&[pkg_name.as_str()]).await.map_err(|e| {
        crate::error::RexebError::Network(format!("AUR lookup failed: {}", e))
    })?;
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
    // Holds a generated temp dir alive until the push finishes (auto-cleaned)
    let mut _temp_guard: Option<tempfile::TempDir> = None;

    // Determine PKGBUILD location
    if input.is_dir() {
        let pb = input.join("PKGBUILD");
        if !pb.exists() {
            return Err(crate::error::RexebError::Validation(format!("No PKGBUILD in {}", input.display())));
        }
        pkgbuild_dir = input.clone();
    } else if input.is_file() {
        // Convert any supported format (.deb/.rpm/.AppImage) to PKGBUILD first
        let temp = tempfile::Builder::new().prefix("rexeb-").tempdir()?;
        let parser = crate::parsers::detect_and_create(input)?;
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
            print_pkgbuild_stub_warning();
            return Ok(());
        }
        _temp_guard = Some(temp);
    } else {
        return Err(crate::error::RexebError::Validation(format!("Input must be a package file (.deb, .rpm, .AppImage) or a directory with a PKGBUILD: {}", input.display())));
    }

    if args.dry_run {
        let content = std::fs::read_to_string(pkgbuild_dir.join("PKGBUILD"))?;
        println!("\n--- PKGBUILD ({} ) ---\n{}", pkgbuild_dir.display(), content);
        let srcinfo_path = pkgbuild_dir.join(".SRCINFO");
        if srcinfo_path.exists() {
            println!("\n--- .SRCINFO ---\n{}", std::fs::read_to_string(&srcinfo_path)?);
        }
        println!("\n{}: would push to AUR as '{}'", style("Dry run").yellow(), args.pkgbase.as_deref().unwrap_or("from PKGBUILD"));
        print_pkgbuild_stub_warning();
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

    // Generate .SRCINFO if missing, preferring makepkg's own printer so the
    // pushed metadata matches what `makepkg --printsrcinfo` would produce
    if !pkgbuild_dir.join(".SRCINFO").exists() {
        let printed = std::process::Command::new("makepkg")
            .arg("--printsrcinfo")
            .current_dir(&pkgbuild_dir)
            .output();
        match printed {
            Ok(output) if output.status.success() => {
                std::fs::write(pkgbuild_dir.join(".SRCINFO"), output.stdout)?;
            }
            _ => {
                let content = std::fs::read_to_string(pkgbuild_dir.join("PKGBUILD"))?;
                let srcinfo = srcinfo_from_pkgbuild_text(&content);
                std::fs::write(pkgbuild_dir.join(".SRCINFO"), srcinfo)?;
            }
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

/// Warn that generated PKGBUILDs are binary-repack stubs, not AUR-ready sources
fn print_pkgbuild_stub_warning() {
    use console::style;
    println!(
        "\n{} rexeb-generated PKGBUILDs are binary-repack stubs without `source=()`.",
        style("Note:").yellow()
    );
    println!("  Before pushing to the AUR, add a `source=()` pointing at the upstream");
    println!("  package file, fill in `sha256sums=()`, and run `makepkg --printsrcinfo > .SRCINFO`.");
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
    for dep in meta
        .get_deps(crate::models::DependencyType::Depends)
        .iter()
        .chain(meta.get_deps(crate::models::DependencyType::PreDepends))
    {
        if dep.is_mapped() {
            s.push_str(&format!("\tdepends = {}\n", dep.to_arch_string()));
        }
    }
    for dep in meta
        .get_deps(crate::models::DependencyType::Recommends)
        .iter()
        .chain(meta.get_deps(crate::models::DependencyType::Suggests))
    {
        if dep.is_mapped() {
            s.push_str(&format!("\toptdepends = {}\n", dep.to_arch_string()));
        }
    }
    s.push_str(&format!("\npkgname = {}\n", meta.effective_name()));
    s
}

/// Minimal PKGBUILD parser used as a `.SRCINFO` fallback
///
/// Only used when `makepkg --printsrcinfo` is unavailable; understands plain
/// `key=value` scalars and single-line `(...)` arrays.
fn srcinfo_from_pkgbuild_text(text: &str) -> String {
    fn scalar(text: &str, key: &str) -> Option<String> {
        let prefix = format!("{}=", key);
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix(prefix.as_str()) {
                return Some(
                    rest.trim()
                        .trim_matches(|c| c == '"' || c == '\'')
                        .to_string(),
                );
            }
        }
        None
    }
    fn array(text: &str, key: &str) -> Vec<String> {
        let prefix = format!("{}=", key);
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix(prefix.as_str()) {
                let rest = rest
                    .trim()
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .trim();
                if rest.is_empty() {
                    return Vec::new();
                }
                let mut out = Vec::new();
                let mut current = String::new();
                let mut quote: Option<char> = None;
                for ch in rest.chars() {
                    if let Some(q) = quote {
                        if ch == q {
                            quote = None;
                        } else {
                            current.push(ch);
                        }
                    } else if ch == '"' || ch == '\'' {
                        quote = Some(ch);
                    } else if ch.is_whitespace() {
                        if !current.is_empty() {
                            out.push(std::mem::take(&mut current));
                        }
                    } else {
                        current.push(ch);
                    }
                }
                if !current.is_empty() {
                    out.push(current);
                }
                return out;
            }
        }
        Vec::new()
    }

    let pkgname = scalar(text, "pkgname")
        .or_else(|| scalar(text, "pkgbase"))
        .unwrap_or_else(|| "rexeb-pkg".to_string());
    let mut s = String::new();
    s.push_str(&format!("pkgbase = {}\n", scalar(text, "pkgbase").unwrap_or_else(|| pkgname.clone())));
    if let Some(v) = scalar(text, "pkgdesc") {
        s.push_str(&format!("\tpkgdesc = {}\n", v));
    }
    s.push_str(&format!(
        "\tpkgver = {}\n",
        scalar(text, "pkgver").unwrap_or_else(|| "0".to_string())
    ));
    s.push_str(&format!(
        "\tpkgrel = {}\n",
        scalar(text, "pkgrel").unwrap_or_else(|| "1".to_string())
    ));
    if let Some(v) = scalar(text, "url") {
        s.push_str(&format!("\turl = {}\n", v));
    }
    for v in array(text, "arch") {
        s.push_str(&format!("\tarch = {}\n", v));
    }
    for v in array(text, "license") {
        s.push_str(&format!("\tlicense = {}\n", v));
    }
    for v in array(text, "depends") {
        s.push_str(&format!("\tdepends = {}\n", v));
    }
    for v in array(text, "optdepends") {
        s.push_str(&format!("\toptdepends = {}\n", v));
    }
    s.push_str(&format!("\npkgname = {}\n", pkgname));
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

    let config = crate::config::Config::load().unwrap_or_default();
    let client = config.http_client()?;

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

    // NOTE: in-place binary replacement is intentionally not implemented
    // (replacing a running binary / system package from inside the tool is
    // unsafe); the commands above are the supported upgrade path.
    println!("\n{} Automatic replacement is not implemented; run one of the update commands above to upgrade.", style("Next steps:").cyan());
    Ok(())
}

/// Compare versions where a release beats its own prereleases
/// (`0.2.4` is newer than `0.2.4-alpha`)
fn is_version_newer(current: &str, latest: &str) -> bool {
    fn split(s: &str) -> (Vec<u64>, Option<&str>) {
        let mut parts = s.splitn(2, '-');
        let core = parts.next().unwrap_or(s);
        let pre = parts.next();
        let nums = core
            .split('.')
            .filter_map(|p| p.parse::<u64>().ok())
            .collect();
        (nums, pre)
    }
    let (cur, cur_pre) = split(current);
    let (lat, lat_pre) = split(latest);
    let width = cur.len().max(lat.len());
    for i in 0..width {
        let a = cur.get(i).copied().unwrap_or(0);
        let b = lat.get(i).copied().unwrap_or(0);
        if b > a {
            return true;
        }
        if a > b {
            return false;
        }
    }
    // Equal numeric core: a release beats a prerelease; between two
    // prereleases the lexicographically later tag wins.
    match (cur_pre, lat_pre) {
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (Some(a), Some(b)) => b > a,
        (None, None) => false,
    }
}
