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
        #[arg(long, conflicts_with = "json_events")]
        raw: bool,

        /// Output mrmouth lifecycle events as JSONL; distinct from --raw inner-agent JSON
        #[arg(long, conflicts_with = "raw")]
        json_events: bool,

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

        /// Output mrmouth lifecycle events as JSONL; disables the TUI
        #[arg(long)]
        json_events: bool,
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

        /// Output mrmouth lifecycle events as JSONL; disables the TUI
        #[arg(long)]
        json_events: bool,
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

        /// Output mrmouth lifecycle events as JSONL; disables the TUI
        #[arg(long)]
        json_events: bool,
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

    // Start TUI unless machine-readable output is selected or stderr is not a TTY.
    let project_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("mrmouth");
    let output_modes = output_modes(&cli.command);
    let lifecycle_command = match &cli.command {
        Commands::Run { .. } => "run",
        Commands::Do { .. } => "do",
        Commands::Loop { .. } => "loop",
        Commands::Ready { .. } => "ready",
        Commands::Summary { .. } => "summary",
        Commands::CodexLogin => "codex-login",
    };
    let lifecycle_item_id = match &cli.command {
        Commands::Do { item_id, .. } => Some(item_id.clone()),
        _ => None,
    };
    let tui = if output_modes.start_tui {
        tui::TuiHandle::try_start(project_name)
    } else {
        None
    };
    let lifecycle_events = lifecycle_event_sink(output_modes.json_events, tui.as_ref());

    let result: Result<(), FailureDebrief> = match cli.command {
        Commands::Run {
            raw,
            json_events: _,
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
                event_sink: lifecycle_events.clone(),
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
            json_events: _,
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
                event_sink: lifecycle_events.clone(),
            };
            loop_cmd::execute(&config, &repo_root, opts, tui.as_ref()).map_err(|e| e.debrief())
        }
        Commands::Do {
            item_id,
            timeout,
            max_failures,
            model,
            json_events: _,
        } => {
            let opts = do_cmd::DoOptions {
                item_id,
                timeout: timeout.unwrap_or(config.do_config.timeout),
                max_failures: max_failures.unwrap_or(config.do_config.max_failures),
                model: resolve_model(&config, model),
                event_sink: lifecycle_events.clone(),
            };
            do_cmd::execute(&config, &repo_root, opts, tui.as_ref()).map_err(|e| e.debrief())
        }
        Commands::Ready {
            timeout,
            max_failures,
            model,
            json_events: _,
        } => {
            let opts = ready::ReadyOptions {
                timeout: timeout.unwrap_or(config.do_config.timeout),
                max_failures: max_failures.unwrap_or(config.do_config.max_failures),
                model: resolve_model(&config, model),
                event_sink: lifecycle_events.clone(),
            };
            ready::execute(&config, &repo_root, opts, tui.as_ref()).map_err(|e| e.debrief())
        }
        Commands::Summary { log_file } => {
            let log_file = log_file.unwrap_or_else(|| format!("{}/latest.jsonl", config.log_dir));
            summary::execute(&config, &repo_root, &log_file, None).map_err(|e| e.debrief())
        }
        Commands::CodexLogin => codex_login::execute(&config, &repo_root).map_err(|e| e.debrief()),
    };

    // Drop TUI first to restore terminal before printing errors or exiting —
    // otherwise the alt-screen teardown erases the debrief.
    drop(tui);

    if let Err(d) = result {
        if let Some(sink) = lifecycle_events {
            let summary = lifecycle_failure_summary(lifecycle_command, lifecycle_item_id, &d);
            sink.emit(crate::events::MrmouthEvent::LifecycleSummary { summary });
        }
        d.print();
        std::process::exit(1);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OutputModes {
    json_events: bool,
    start_tui: bool,
}

fn output_modes(command: &Commands) -> OutputModes {
    let raw = matches!(
        command,
        Commands::Run { raw: true, .. } | Commands::Summary { .. }
    );
    let json_events = matches!(
        command,
        Commands::Run {
            json_events: true,
            ..
        } | Commands::Do {
            json_events: true,
            ..
        } | Commands::Loop {
            json_events: true,
            ..
        } | Commands::Ready {
            json_events: true,
            ..
        }
    );

    OutputModes {
        json_events,
        start_tui: !raw && !json_events,
    }
}

fn lifecycle_event_sink(
    json_events: bool,
    tui: Option<&tui::TuiHandle>,
) -> Option<crate::events::EventSinkHandle> {
    let mut sinks = Vec::new();

    if let Some(tui) = tui {
        sinks.push(crate::events::EventSinkHandle::new(tui.event_sink()));
    }
    if json_events {
        sinks.push(crate::events::EventSinkHandle::new(
            crate::events::JsonlEventSink::stdout(),
        ));
    }

    match sinks.len() {
        0 => None,
        1 => sinks.pop(),
        _ => Some(crate::events::EventSinkHandle::fanout(sinks)),
    }
}

fn lifecycle_failure_summary(
    command: &str,
    item_id: Option<String>,
    d: &FailureDebrief,
) -> crate::events::LifecycleSummary {
    let mut summary = crate::events::LifecycleSummary::failed(command, d.message.clone())
        .next_action("inspect_error");
    if let Some(item_id) = item_id {
        summary = summary.item_id(item_id);
    }
    if let Some(exit_code) = d.exit_code {
        summary = summary.exit_code(exit_code);
    }
    if let Some(log_path) = &d.log_path {
        summary = summary.log_path(log_path.display().to_string());
        if let Some(jsonl_path) = inferred_jsonl_path(log_path) {
            summary = summary.jsonl_path(jsonl_path.display().to_string());
        }
    }
    summary
}

fn inferred_jsonl_path(log_path: &std::path::Path) -> Option<std::path::PathBuf> {
    (log_path.extension().and_then(|ext| ext.to_str()) == Some("log"))
        .then(|| log_path.with_extension("jsonl"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn run_raw_and_json_events_conflict() {
        let err = match Cli::try_parse_from(["mrmouth", "run", "--raw", "--json-events"]) {
            Ok(_) => panic!("expected --raw and --json-events to conflict"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn json_events_mode_disables_tui_for_run() {
        let command = Commands::Run {
            raw: false,
            json_events: true,
            model: None,
            timeout: None,
            local: false,
        };

        assert_eq!(
            output_modes(&command),
            OutputModes {
                json_events: true,
                start_tui: false,
            }
        );
    }

    #[test]
    fn json_events_mode_disables_tui_for_do() {
        let command = Commands::Do {
            item_id: "lb-1234".to_string(),
            timeout: None,
            max_failures: None,
            model: None,
            json_events: true,
        };

        assert_eq!(
            output_modes(&command),
            OutputModes {
                json_events: true,
                start_tui: false,
            }
        );
    }

    #[test]
    fn json_events_mode_disables_tui_for_loop() {
        let command = Commands::Loop {
            delay: 0,
            max_runs: None,
            no_summary: false,
            model: None,
            json_events: true,
        };

        assert_eq!(
            output_modes(&command),
            OutputModes {
                json_events: true,
                start_tui: false,
            }
        );
    }

    #[test]
    fn json_events_mode_disables_tui_for_ready() {
        let command = Commands::Ready {
            timeout: None,
            max_failures: None,
            model: None,
            json_events: true,
        };

        assert_eq!(
            output_modes(&command),
            OutputModes {
                json_events: true,
                start_tui: false,
            }
        );
    }

    #[test]
    fn raw_mode_disables_tui_without_lifecycle_json() {
        let command = Commands::Run {
            raw: true,
            json_events: false,
            model: None,
            timeout: None,
            local: false,
        };

        assert_eq!(
            output_modes(&command),
            OutputModes {
                json_events: false,
                start_tui: false,
            }
        );
    }

    #[test]
    fn failure_summary_includes_item_exit_and_inferred_jsonl_path() {
        let mut debrief = FailureDebrief::new("container failed".to_string());
        debrief.exit_code = Some(17);
        debrief.log_path = Some(PathBuf::from("/repo/logs/run-1.log"));

        let summary = lifecycle_failure_summary("do", Some("lb-1234".to_string()), &debrief);
        let event = crate::events::MrmouthEvent::LifecycleSummary { summary };

        assert_eq!(
            event.to_json_value().unwrap(),
            json!({
                "type": "lifecycle_summary",
                "summary": {
                    "status": "failed",
                    "command": "do",
                    "item_id": "lb-1234",
                    "log_path": "/repo/logs/run-1.log",
                    "jsonl_path": "/repo/logs/run-1.jsonl",
                    "exit_code": 17,
                    "failure": "container failed",
                    "next_action": "inspect_error",
                }
            })
        );
    }
}
