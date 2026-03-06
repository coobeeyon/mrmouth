mod config;
mod docker;
mod epic;
mod init;
mod litebrite;
mod loop_cmd;
mod prompt;
mod run;
pub mod stream_fmt;
mod summary;

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
    },

    /// Work through a litebrite epic's tasks sequentially
    Epic {
        /// The litebrite epic ID
        epic_id: String,

        /// Per-task timeout in minutes
        #[arg(long, default_value_t = 15)]
        timeout: u32,

        /// Consecutive failures before aborting
        #[arg(long, default_value_t = 3)]
        max_failures: u32,
    },

    /// Scaffold .mrmouth/ config in the current repo
    Init,

    /// Generate an AI summary of a run log
    Summary {
        /// Path to log file (default: logs/latest.jsonl)
        log_file: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    // Init doesn't need config (it creates it)
    if matches!(cli.command, Commands::Init) {
        if let Err(e) = init::execute() {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }

    let is_local = matches!(cli.command, Commands::Run { local: true, .. });
    let repo_root = if is_local {
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

    match cli.command {
        Commands::Run { raw, model, timeout, local } => {
            let opts = run::RunOptions {
                raw,
                model: model.unwrap_or_else(|| config.model.clone()),
                timeout,
                local,
                prompt_override: None,
            };
            if let Err(e) = run::execute(&config, &repo_root, opts) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Loop { delay, max_runs, no_summary } => {
            let opts = loop_cmd::LoopOptions {
                delay: if delay > 0 { delay } else { config.loop_config.delay },
                max_runs: max_runs.unwrap_or(config.loop_config.max_runs),
                no_summary,
                model: config.model.clone(),
            };
            if let Err(e) = loop_cmd::execute(&config, &repo_root, opts) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Epic { epic_id, timeout, max_failures } => {
            let opts = epic::EpicOptions {
                epic_id,
                timeout: if timeout != 15 { timeout } else { config.epic.timeout },
                max_failures: if max_failures != 3 { max_failures } else { config.epic.max_failures },
                model: config.model.clone(),
            };
            if let Err(e) = epic::execute(&config, &repo_root, opts) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Init => unreachable!(),
        Commands::Summary { log_file } => {
            let log_file = log_file.unwrap_or_else(|| {
                format!("{}/latest.jsonl", config.log_dir)
            });
            if let Err(e) = summary::execute(&config, &repo_root, &log_file) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}
