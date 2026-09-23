//! Rexeb - A smarter, faster debtap alternative
//!
//! Main entry point for the rexeb CLI application.

use std::process::ExitCode;

use console::style;
use tracing_subscriber::EnvFilter;

use rexeb::cli::{self, Cli, Commands};
use rexeb::error::Result;

/// Application banner
const BANNER: &str = r#"
  ██████╗ ███████╗██╗  ██╗███████╗██████╗ 
  ██╔══██╗██╔════╝╚██╗██╔╝██╔════╝██╔══██╗
  ██████╔╝█████╗   ╚███╔╝ █████╗  ██████╔╝
  ██╔══██╗██╔══╝   ██╔██╗ ██╔══╝  ██╔══██╗
  ██║  ██║███████╗██╔╝ ██╗███████╗██████╔╝
  ╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚══════╝╚═════╝ 
"#;

#[tokio::main]
async fn main() -> ExitCode {
    // Parse CLI arguments
    let cli = Cli::parse_args();

    // Set up logging
    setup_logging(&cli);

    // Run the application
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{} {}", style("Error:").red().bold(), e);
            ExitCode::FAILURE
        }
    }
}

/// Set up logging based on CLI arguments (falling back to the config file)
fn setup_logging(cli: &Cli) {
    let config = rexeb::config::Config::load().unwrap_or_default();
    let level: &str = if cli.verbose {
        "debug"
    } else if cli.quiet {
        "error"
    } else {
        config.logging.level.as_str()
    };

    // RUST_LOG wins when set; a bogus configured level falls back to info
    // instead of panicking inside EnvFilter::new.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info")));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .with_ansi(config.logging.color && !cli.quiet)
        .init();
}

/// Main application logic
async fn run(cli: Cli) -> Result<()> {
    // Honor --config everywhere by bridging it into REXEB_CONFIG, which
    // Config::config_path() consults (clap only reads the env var, so an
    // explicit flag would otherwise be ignored by every Config::load()).
    if let Some(ref config_path) = cli.config {
        std::env::set_var("REXEB_CONFIG", config_path);
    }

    // Show banner for main commands (not quiet mode)
    if !cli.quiet {
        match &cli.command {
            Commands::Convert(_) | Commands::Install(_) => {
                println!("{}", style(BANNER).cyan());
                println!("  {} v{}\n",
                    style("rexeb").bold(),
                    style(rexeb::VERSION).dim()
                );
            }
            _ => {}
        }
    }

    // Set number of parallel jobs (--jobs > config > low-RAM/low-core caps)
    let config_jobs = rexeb::config::Config::load()
        .ok()
        .and_then(|c| c.general.jobs);
    let effective_jobs = rexeb::effective_parallel_jobs(cli.jobs.or(config_jobs));
    rayon::ThreadPoolBuilder::new()
        .num_threads(effective_jobs)
        .build_global()
        .ok();
    tracing::debug!("Using {} parallel jobs", effective_jobs);

    // Handle TUI mode: run the real conversion with progress reporting
    #[cfg(feature = "tui")]
    if cli.tui {
        use rexeb::tui::{run_tui, App, ProgressEvent};
        use tokio::sync::mpsc;

        match &cli.command {
            Commands::Convert(args) => {
                let app = App::new();
                let tick_rate = std::time::Duration::from_millis(250);
                let (tx, rx) = mpsc::channel::<ProgressEvent>(64);
                let convert_args = args.clone();

                tokio::spawn(async move {
                    let total = convert_args.input.len().max(1);
                    let _ = tx
                        .send(ProgressEvent::Status("Starting conversion...".into()))
                        .await;
                    for (i, input) in convert_args.input.iter().enumerate() {
                        let _ = tx
                            .send(ProgressEvent::Log(format!("Converting {} ...", input.display())))
                            .await;
                        let single = rexeb::cli::ConvertArgs {
                            input: vec![input.clone()],
                            ..convert_args.clone()
                        };
                        match rexeb::cli::execute_convert(&single, true, None).await {
                            Ok(()) => {
                                let _ = tx
                                    .send(ProgressEvent::Log(format!("Done: {}", input.display())))
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx.send(ProgressEvent::Error(e.to_string())).await;
                                return;
                            }
                        }
                        let _ = tx
                            .send(ProgressEvent::Progress((i + 1) as f64 / total as f64))
                            .await;
                    }
                    let _ = tx.send(ProgressEvent::Done).await;
                });

                run_tui(app, tick_rate, rx).await?;
                return Ok(());
            }
            _ => {
                eprintln!("--tui currently supports only the `convert` command; running normally.");
            }
        }
    }

    #[cfg(not(feature = "tui"))]
    if cli.tui {
        eprintln!("Warning: --tui was passed but rexeb was built without the `tui` feature; ignoring.");
    }

    // Dispatch to appropriate command handler
    match cli.command {
        Commands::Convert(args) => {
            cli::execute_convert(&args, cli.quiet, cli.jobs).await
        }
        Commands::Update(args) => {
            cli::execute_update(&args, cli.quiet).await
        }
        Commands::Info(args) => {
            cli::execute_info(&args).await
        }
        Commands::Search(args) => {
            cli::execute_search(&args).await
        }
        Commands::Analyze(args) => {
            cli::execute_analyze(&args).await
        }
        Commands::Install(args) => {
            cli::execute_install(&args, cli.quiet).await
        }
        Commands::Config(args) => {
            cli::execute_config(&args).await
        }
        Commands::Clean(args) => {
            cli::execute_clean(&args).await
        }
        Commands::Map(args) => {
            cli::execute_map(&args).await
        }
        Commands::CheckAur(args) => {
            cli::execute_check_aur(&args).await
        }
        Commands::AurPush(args) => {
            cli::execute_aur_push(&args).await
        }
        Commands::ListInstalled(args) => {
            cli::execute_list_installed(&args).await
        }
        Commands::Manage(args) => {
            cli::execute_manage(&args).await
        }
        Commands::SelfUpdate(args) => {
            cli::execute_self_update(&args).await
        }
        Commands::Completions(args) => {
            cli::execute_completions(&args).await
        }
        Commands::Manpage(args) => {
            cli::execute_manpage(&args).await
        }
        Commands::SandboxBuild(args) => {
            cli::execute_sandbox_build(&args)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_banner() {
        // The banner should contain the logo with the name "rexeb" in it
        // The banner is ASCII art, so we check that it's not empty and has the expected structure
        assert!(!BANNER.trim().is_empty());
        assert!(BANNER.lines().count() >= 6); // The logo has 6 lines
    }
}
