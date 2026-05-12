mod agent;
mod codex_login;
mod config;
mod debrief;
mod do_cmd;
mod docker;
pub mod events;
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
use debrief::FailureDebrief;

const CLAUDE_DEFAULT_MODEL: &str = "opus";

#[derive(Parser)]
#[command(
    name = "mrmouth",
    version,
    about = "Run Claude Code or Codex as an autonomous coding agent in Docker containers"
)]
struct Cli {
    /// Run all AI roles with Claude Code, overriding config
    #[arg(long, global = true, conflicts_with = "codex")]
    claude: bool,

    /// Run all AI roles with Codex, overriding config
    #[arg(long, global = true)]
    codex: bool,

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

        /// Override the agent model (default: from config)
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

        /// Override the agent model (default: from config)
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

        /// Override the agent model (default: from config)
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

        /// Override the agent model (default: from config)
        #[arg(long)]
        model: Option<String>,
    },

    /// Generate an AI summary of a run log
    Summary {
        /// Path to log file (default: logs/latest.jsonl)
        log_file: Option<String>,
    },

    /// Sign in to Codex inside mrmouth's persisted Docker auth volume
    CodexLogin,
}

fn main() {
    let cli = Cli::parse();

    let use_cwd_fallback = matches!(
        cli.command,
        Commands::Run { local: true, .. }
            | Commands::Loop { .. }
            | Commands::Summary { .. }
            | Commands::CodexLogin
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

    let mut config = match Config::load(&repo_root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    if cli.codex {
        config.agent = crate::agent::AgentKind::Codex;
    } else if cli.claude {
        config.agent = crate::agent::AgentKind::Claude;
    }

    // Start TUI unless --raw is set or stderr is not a TTY
    let project_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("mrmouth");
    let use_raw = matches!(
        cli.command,
        Commands::Run { raw: true, .. } | Commands::Summary { .. }
    );
    let tui = if use_raw {
        None
    } else {
        tui::TuiHandle::try_start(project_name)
    };

    let result: Result<(), FailureDebrief> = match cli.command {
        Commands::Run {
            raw,
            model,
            timeout,
            local,
        } => {
            let opts = run::RunOptions {
                raw,
                model: resolve_model(&config, model),
                timeout,
                local,
                prompt_override: None,
                branch: None,
            };
            run::execute(&config, &repo_root, opts, tui.as_ref())
                .map(|_| ())
                .map_err(|e| e.debrief())
        }
        Commands::Loop {
            delay,
            max_runs,
            no_summary,
            model,
        } => {
            let opts = loop_cmd::LoopOptions {
                delay: if delay > 0 {
                    delay
                } else {
                    config.loop_config.delay
                },
                max_runs: max_runs.unwrap_or(config.loop_config.max_runs),
                no_summary,
                model: resolve_model(&config, model),
            };
            loop_cmd::execute(&config, &repo_root, opts, tui.as_ref()).map_err(|e| e.debrief())
        }
        Commands::Do {
            item_id,
            timeout,
            max_failures,
            model,
        } => {
            let opts = do_cmd::DoOptions {
                item_id,
                timeout: timeout.unwrap_or(config.do_config.timeout),
                max_failures: max_failures.unwrap_or(config.do_config.max_failures),
                model: resolve_model(&config, model),
            };
            do_cmd::execute(&config, &repo_root, opts, tui.as_ref()).map_err(|e| e.debrief())
        }
        Commands::Ready {
            timeout,
            max_failures,
            model,
        } => {
            let opts = ready::ReadyOptions {
                timeout: timeout.unwrap_or(config.do_config.timeout),
                max_failures: max_failures.unwrap_or(config.do_config.max_failures),
                model: resolve_model(&config, model),
            };
            ready::execute(&config, &repo_root, opts, tui.as_ref()).map_err(|e| e.debrief())
        }
        Commands::Summary { log_file } => {
            let log_file = log_file.unwrap_or_else(|| format!("{}/latest.jsonl", config.log_dir));
            summary::execute(&config, &repo_root, &log_file, None).map_err(|e| e.debrief())
        }
        Commands::CodexLogin => {
            codex_login::execute(&config, &repo_root).map_err(|e| e.debrief())
        }
    };

    // Drop TUI first to restore terminal before printing errors or exiting —
    // otherwise the alt-screen teardown erases the debrief.
    drop(tui);

    if let Err(d) = result {
        d.print();
        std::process::exit(1);
    }
}

fn resolve_model(config: &Config, override_model: Option<String>) -> String {
    if let Some(model) = override_model {
        return model;
    }
    if config.agent == crate::agent::AgentKind::Codex && config.model == CLAUDE_DEFAULT_MODEL {
        return String::new();
    }
    config.model.clone()
}
