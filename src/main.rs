mod config;
mod docker;
mod do_cmd;
mod litebrite;
mod logger;
mod loop_cmd;
mod prompt;
mod ready;
mod reviewer;
mod run;
mod shipper;
pub mod stream_fmt;
mod streaming;
mod summary;
mod tui;

use clap::{Parser, Subcommand};
use config::Config;

#[derive(Parser)]
#[command(name = "mrmouth", version, about = "Run Claude Code as an autonomous coding agent in Docker containers")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run one agent session
    Run {
        /// Output raw JSONL instead of formatted stream
        #[arg(long)]
        raw: bool,

        /// Override the Claude model (default: from config or opus)
        #[arg(long)]
        model: Option<String>,

        /// Kill container after N minutes
        #[arg(long)]
        timeout: Option<u32>,

        /// Bind-mount current directory instead of cloning
        #[arg(long)]
        local: bool,
    },

    /// Run the agent repeatedly until work is done
    Loop {
        /// Wait between runs in seconds
        #[arg(long, default_value_t = 0)]
        delay: u32,

        /// Stop after N runs regardless of decider
        #[arg(long)]
        max_runs: Option<u32>,

        /// Skip AI summary generation
        #[arg(long)]
        no_summary: bool,

        /// Override the Claude model (default: from config or opus)
        #[arg(long)]
        model: Option<String>,
    },

    /// Work through a litebrite item's tasks sequentially
    Do {
        /// The litebrite item ID
        item_id: String,

        /// Per-task timeout in minutes (default: from config or 15)
        #[arg(long)]
        timeout: Option<u32>,

        /// Consecutive failures before aborting (default: from config or 3)
        #[arg(long)]
        max_failures: Option<u32>,

        /// Override the Claude model (default: from config or opus)
        #[arg(long)]
        model: Option<String>,
    },

    /// Pick up ready items from litebrite and work through them
    Ready {
        /// Per-task timeout in minutes (default: from config or 15)
        #[arg(long)]
        timeout: Option<u32>,

        /// Consecutive failures before aborting (default: from config or 3)
        #[arg(long)]
        max_failures: Option<u32>,

        /// Override the Claude model (default: from config or opus)
        #[arg(long)]
        model: Option<String>,
    },

    /// Generate an AI summary of a run log
    Summary {
        /// Path to log file (default: logs/latest.jsonl)
        log_file: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    let use_cwd_fallback = matches!(
        cli.command,
        Commands::Run { local: true, .. } | Commands::Loop { .. } | Commands::Summary { .. }
    );
    let repo_root = if use_cwd_fallback {
        match Config::find_repo_root_or_cwd() {
            Ok(root) => root,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        match Config::find_repo_root() {
            Ok(root) => root,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    };

    let config = match Config::load(&repo_root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    // Start TUI unless --raw is set or stderr is not a TTY
    let project_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("mrmouth");
    let use_raw = matches!(cli.command, Commands::Run { raw: true, .. } | Commands::Summary { .. });
    let tui = if use_raw {
        None
    } else {
        tui::TuiHandle::try_start(project_name)
    };

    let result = match cli.command {
        Commands::Run { raw, model, timeout, local } => {
            let opts = run::RunOptions {
                raw,
                model: model.unwrap_or_else(|| config.model.clone()),
                timeout,
                local,
                prompt_override: None,
                branch: None,
            };
            run::execute(&config, &repo_root, opts, tui.as_ref()).map(|_| ()).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        }
        Commands::Loop { delay, max_runs, no_summary, model } => {
            let opts = loop_cmd::LoopOptions {
                delay: if delay > 0 { delay } else { config.loop_config.delay },
                max_runs: max_runs.unwrap_or(config.loop_config.max_runs),
                no_summary,
                model: model.unwrap_or_else(|| config.model.clone()),
            };
            loop_cmd::execute(&config, &repo_root, opts, tui.as_ref()).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        }
        Commands::Do { item_id, timeout, max_failures, model } => {
            let opts = do_cmd::DoOptions {
                item_id,
                timeout: timeout.unwrap_or(config.do_config.timeout),
                max_failures: max_failures.unwrap_or(config.do_config.max_failures),
                model: model.unwrap_or_else(|| config.model.clone()),
            };
            do_cmd::execute(&config, &repo_root, opts, tui.as_ref()).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        }
        Commands::Ready { timeout, max_failures, model } => {
            let opts = ready::ReadyOptions {
                timeout: timeout.unwrap_or(config.do_config.timeout),
                max_failures: max_failures.unwrap_or(config.do_config.max_failures),
                model: model.unwrap_or_else(|| config.model.clone()),
            };
            ready::execute(&config, &repo_root, opts, tui.as_ref()).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        }
        Commands::Summary { log_file } => {
            let log_file = log_file.unwrap_or_else(|| {
                format!("{}/latest.jsonl", config.log_dir)
            });
            summary::execute(&config, &repo_root, &log_file, None).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        }
    };

    // Drop TUI first to restore terminal before printing errors or exiting
    drop(tui);

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
