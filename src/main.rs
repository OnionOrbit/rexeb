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

/// Set up logging based on CLI arguments
fn setup_logging(cli: &Cli) {
    let level = if cli.verbose {
        "debug"
    } else if cli.quiet {
        "error"
    } else {
        "info"
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();
}

/// Main application logic
async fn run(cli: Cli) -> Result<()> {
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

    // Set number of parallel jobs
    if let Some(jobs) = cli.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build_global()
            .ok();
    }

    // Handle TUI mode
    #[cfg(feature = "tui")]
    if cli.tui {
        use rexeb::tui::{App, ProgressEvent, run_tui};
        use tokio::sync::mpsc;

        let app = App::new();
        let tick_rate = std::time::Duration::from_millis(250);
        let (tx, rx) = mpsc::channel::<ProgressEvent>(64);

        // Spawn the convert task with progress reporting
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            // For now, demonstrate TUI with a simple test workflow.
            // A full integration would parse the CLI args and run the convert flow here.
            let _ = tx_clone.send(ProgressEvent::Status("Starting conversion...".into())).await;
            let _ = tx_clone.send(ProgressEvent::Log("rexeb TUI mode active".into())).await;
            let _ = tx_clone.send(ProgressEvent::Log("Use this mode for real-time conversion monitoring".into())).await;
            
            for i in 0..=10 {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let pct = i as f64 / 10.0;
                let _ = tx_clone.send(ProgressEvent::Progress(pct)).await;
                let _ = tx_clone.send(ProgressEvent::Log(format!("Step {}/10 complete", i))).await;
            }
            
            let _ = tx_clone.send(ProgressEvent::Done).await;
        });

        run_tui(app, tick_rate, rx).await?;
        return Ok(());
    }

    // Dispatch to appropriate command handler
    match cli.command {
        Commands::Convert(args) => {
            cli::execute_convert(&args).await
        }
        Commands::Update(args) => {
            cli::execute_update(&args).await
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
            cli::execute_install(&args).await
        }
        Commands::Config(args) => {
            cli::execute_config(&args).await
        }
        Commands::Clean(args) => {
            cli::execute_clean(&args).await
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
