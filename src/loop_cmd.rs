use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::events::{
    EventSinkHandle, FinishStatus, LifecycleSummary, MrmouthEvent, ReviewerAction, SyncAction,
    SyncTool,
};
use crate::litebrite;
use crate::logger::Logger;
use crate::repo_layout::RepoLayout;
use crate::reviewer;
use crate::run::{self, RunOptions};
use crate::shipper;
use crate::stream_fmt::StreamFormatter;
use crate::streaming::{self, StreamTarget};
use crate::summary;
use crate::tui::{TuiHandle, TuiSender};

pub struct LoopOptions {
    pub delay: u32,
    pub max_runs: u32,
    pub no_summary: bool,
    pub model: String,
    pub json_events: bool,
    pub event_sink: Option<EventSinkHandle>,
}

struct LoopTerminal {
    finished: Option<(FinishStatus, Option<String>)>,
    summary: LifecycleSummary,
}

impl LoopTerminal {
    fn cancelled(branch: &str) -> Self {
        Self {
            finished: Some((FinishStatus::Cancelled, Some("loop cancelled".to_string()))),
            summary: LifecycleSummary {
                status: FinishStatus::Cancelled,
                command: "loop".to_string(),
                item_id: None,
                branch: Some(branch.to_string()),
                workspace: None,
                commit_range: None,
                log_path: None,
                jsonl_path: None,
                exit_code: None,
                failure: None,
                reviewer: None,
                shipper: None,
                next_action: Some("cancelled".to_string()),
            },
        }
    }

    fn success(summary: LifecycleSummary, finished_summary: Option<&str>) -> Self {
        Self {
            finished: finished_summary
                .map(|summary| (FinishStatus::Success, Some(summary.to_string()))),
            summary,
        }
    }
}

/// Route a message to the TUI pane if available, otherwise stderr.
fn emit(tui_tx: &Option<TuiSender>, msg: &str) {
    match tui_tx {
        Some(sender) => sender.send_line(msg),
        None => eprintln!("{msg}"),
    }
}

pub fn execute(
    config: &Config,
    repo_root: &Path,
    opts: LoopOptions,
    tui: Option<&TuiHandle>,
) -> Result<(), LoopError> {
    // Use the same TUI pane name as run::execute so all output is visible
    // in one place — the user watches "AGENT SESSION" during runs and should
    // see post-run activity (reviewer, decider, etc.) on the same pane.
    let tui_tx = tui.map(|t| t.sender("AGENT SESSION"));
    let repo_layout = RepoLayout::resolve(config, repo_root, None)
        .map_err(|e| LoopError::Bootstrap(e.to_string()))?;
    emit_event(
        &opts.event_sink,
        MrmouthEvent::StageChanged {
            stage: "Loop".to_string(),
        },
    );

    // Create a loop-level logger so that sub-calls (shipper, reviewer, decider)
    // route output through the TUI instead of falling back to eprintln.
    let log_dir = repo_root.join(&config.log_dir);
    let _ = std::fs::create_dir_all(&log_dir);
    let loop_logger = match tui {
        Some(t) => Logger::with_display_sink(&log_dir.join("loop.log"), t.sender("AGENT SESSION")),
        None => Logger::new(&log_dir.join("loop.log")),
    }
    .ok();

    // Cold-start: no git repo yet — init one and run in local (bind-mount) mode
    let bootstrap_mode = !repo_root.join(".git").exists();
    if bootstrap_mode {
        emit(&tui_tx, "BOOTSTRAP");
        emit_event(
            &opts.event_sink,
            MrmouthEvent::StageChanged {
                stage: "Bootstrap".to_string(),
            },
        );
        emit(
            &tui_tx,
            &format!(
                "No git repository found in {}. Running git init...",
                repo_root.display()
            ),
        );
        let init_output = Command::new("git")
            .arg("init")
            .current_dir(repo_root)
            .output()
            .map_err(|e| LoopError::Bootstrap(format!("failed to run git init: {e}")))?;
        if !init_output.status.success() {
            let stderr = String::from_utf8_lossy(&init_output.stderr);
            emit(&tui_tx, &format!("git init failed: {}", stderr.trim()));
            return Err(LoopError::Bootstrap("git init failed".into()));
        }
        let gitignore_path = repo_root.join(".gitignore");
        if !gitignore_path.exists() {
            let _ = std::fs::write(&gitignore_path, "logs/\n");
        } else if let Ok(contents) = std::fs::read_to_string(&gitignore_path) {
            if !contents.lines().any(|l| l.trim() == "logs/") {
                let mut updated = contents;
                if !updated.ends_with('\n') {
                    updated.push('\n');
                }
                updated.push_str("logs/\n");
                let _ = std::fs::write(&gitignore_path, updated);
            }
        }

        // Seed default Dockerfile so the decider has a base to add layers to
        let dockerfile_path = repo_root.join(&config.dockerfile);
        if !dockerfile_path.exists() {
            if let Some(parent) = dockerfile_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&dockerfile_path, crate::docker::DEFAULT_DOCKERFILE);
        }

        let add_status = Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo_root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| LoopError::Bootstrap(format!("failed to stage files: {e}")))?;
        if add_status.success() {
            let has_staged = Command::new("git")
                .args(["diff", "--cached", "--quiet"])
                .current_dir(repo_root)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| !s.success())
                .unwrap_or(false);
            if has_staged {
                let commit_output = Command::new("git")
                    .args(["commit", "-m", "Initial commit"])
                    .current_dir(repo_root)
                    .output()
                    .map_err(|e| {
                        LoopError::Bootstrap(format!("failed to commit initial files: {e}"))
                    })?;
                if !commit_output.status.success() {
                    let stderr = String::from_utf8_lossy(&commit_output.stderr);
                    emit(
                        &tui_tx,
                        &format!("Initial commit failed: {}", stderr.trim()),
                    );
                    return Err(LoopError::Bootstrap("initial commit failed".into()));
                }
            }
        }
    }

    // Pre-initialized repo with zero commits: seed one so branches can be
    // created and cloned.  This covers the case where the user ran `git init`
    // (and possibly `lb init`) before invoking mrmouth.
    let has_commits = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(repo_root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_commits {
        emit(&tui_tx, "No commits found — creating seed commit");
        let gitignore_path = repo_root.join(".gitignore");
        if !gitignore_path.exists() {
            let _ = std::fs::write(&gitignore_path, "logs/\n");
        }
        let dockerfile_path = repo_root.join(&config.dockerfile);
        if !dockerfile_path.exists() {
            if let Some(parent) = dockerfile_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&dockerfile_path, crate::docker::DEFAULT_DOCKERFILE);
        }
        let _ = Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo_root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let seed_status = Command::new("git")
            .args(["commit", "--allow-empty", "-m", "Initial commit"])
            .current_dir(repo_root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| LoopError::Bootstrap(format!("failed to create seed commit: {e}")))?;
        if !seed_status.success() {
            return Err(LoopError::Bootstrap("seed commit failed".into()));
        }
    }

    // Capture parent branch before creating feature branch
    let parent_branch = git_current_branch(repo_root).unwrap_or_else(|_| "main".into());

    // Create feature branch (unless bootstrap mode — stay on main)
    let current_branch = if bootstrap_mode {
        parent_branch.clone()
    } else {
        emit(&tui_tx, "BRANCH SETUP");
        emit_event(
            &opts.event_sink,
            MrmouthEvent::StageChanged {
                stage: "Branch setup".to_string(),
            },
        );
        let branch_name = shipper::generate_branch_name(
            config,
            repo_root,
            &config.effective_model_for_agent(&config.loop_config.shipper_model),
            loop_logger.as_ref(),
        )
        .map_err(|e| LoopError::BranchCreation(format!("failed to generate branch name: {e}")))?;
        shipper::create_and_push_branch(
            repo_root,
            &branch_name,
            loop_logger.as_ref(),
            &opts.event_sink,
        )
        .map_err(|e| LoopError::BranchCreation(format!("failed to create branch: {e}")))?;
        branch_name
    };

    let max_label = if opts.max_runs == 0 {
        "unlimited".to_string()
    } else {
        opts.max_runs.to_string()
    };
    emit(
        &tui_tx,
        &format!(
            "Agent loop: {}s between runs, max={}, Ctrl-C to stop",
            opts.delay, max_label
        ),
    );

    // Boot a long-lived session container, reused across iterations. Setup
    // (clone, lb init, claude.json restore) runs once; each iteration execs
    // task.sh instead of starting a fresh container. The loop_logger here is
    // required — we need a place to stream session setup output.
    let session_logger = loop_logger
        .as_ref()
        .ok_or_else(|| LoopError::Bootstrap("loop logger missing — cannot start session".into()))?;
    let session_log_path = log_dir.join("session.log");
    emit_event(
        &opts.event_sink,
        MrmouthEvent::StageChanged {
            stage: "Session setup".to_string(),
        },
    );
    emit(&tui_tx, "Starting session container...");
    let mut session = run::start_session(
        config,
        repo_root,
        &current_branch,
        false,
        None,
        repo_layout.docker_work_mount(),
        tui,
        session_logger,
        &session_log_path,
    )
    .map_err(|e| LoopError::SessionStart(Box::new(e)))?;

    let mut run_number: u32 = 0;
    let mut terminal: Option<LoopTerminal> = None;

    let loop_result = (|| -> Result<(), LoopError> {
        loop {
            run_number += 1;

            // Check if TUI user cancelled
            if tui.is_some_and(|t| t.is_cancelled()) {
                emit(&tui_tx, "LOOP CANCELLED BY USER");
                terminal = Some(LoopTerminal::cancelled(&current_branch));
                break;
            }

            if opts.max_runs > 0 && run_number > opts.max_runs {
                emit(&tui_tx, "");
                emit(&tui_tx, &format!("LOOP COMPLETE  {} runs", opts.max_runs));
                terminal = Some(LoopTerminal::success(
                    LifecycleSummary::success("loop")
                        .branch(current_branch.clone())
                        .next_action("max_runs_reached"),
                    Some("max runs reached"),
                ));
                break;
            }

            // --- Decider (runs first; uses loop_logger since no run logger exists yet) ---
            emit_event(
                &opts.event_sink,
                MrmouthEvent::StageChanged {
                    stage: "Deciding".to_string(),
                },
            );
            let decider_model = config.effective_model_for_agent(&config.loop_config.decider_model);
            let role_start = std::time::Instant::now();
            let decision = should_continue(
                config,
                repo_root,
                &decider_model,
                loop_logger.as_ref(),
                &log_dir,
            );
            crate::logger::log_timing(loop_logger.as_ref(), "decider-wall", role_start.elapsed());

            match decision {
                Ok(Decision::Continue(reason)) => {
                    crate::logger::log(
                        loop_logger.as_ref(),
                        &format!("Decider: continue — {reason}"),
                    );
                }
                Ok(Decision::Ship(reason)) => {
                    crate::logger::log(loop_logger.as_ref(), &format!("Decider: ship — {reason}"));

                    emit_event(
                        &opts.event_sink,
                        MrmouthEvent::StageChanged {
                            stage: "Shipper".to_string(),
                        },
                    );
                    let ship_opts = shipper::ShipperOptions {
                        model: config.effective_model_for_agent(&config.loop_config.shipper_model),
                        current_branch: current_branch.clone(),
                        parent_branch: parent_branch.clone(),
                        event_sink: opts.event_sink.clone(),
                    };

                    let role_start = std::time::Instant::now();
                    let ship_result =
                        shipper::execute(config, repo_root, &ship_opts, loop_logger.as_ref());
                    crate::logger::log_timing(
                        loop_logger.as_ref(),
                        "shipper-wall",
                        role_start.elapsed(),
                    );

                    match ship_result {
                        Ok(()) => {
                            crate::logger::log(
                                loop_logger.as_ref(),
                                "Shipped! Merged to parent branch.",
                            );
                            let completed = run_number - 1;
                            emit(
                                &tui_tx,
                                &format!("LOOP COMPLETE  {completed} runs (shipped)"),
                            );
                            terminal = Some(LoopTerminal::success(
                                LifecycleSummary::success("loop")
                                    .branch(current_branch.clone())
                                    .shipper("shipped")
                                    .next_action("merged"),
                                None,
                            ));
                            break;
                        }
                        Err(e) => {
                            crate::logger::log(
                                loop_logger.as_ref(),
                                &format!("Ship failed (continuing on current branch): {e}"),
                            );
                            emit_event(
                                &opts.event_sink,
                                MrmouthEvent::failure("ship failed", None, Some(e.to_string())),
                            );
                        }
                    }
                }
                Ok(Decision::Stop(reason)) => {
                    crate::logger::log(loop_logger.as_ref(), &format!("Decider: stop — {reason}"));
                    let completed = run_number - 1;
                    emit(&tui_tx, &format!("LOOP COMPLETE  {completed} runs"));
                    terminal = Some(LoopTerminal::success(
                        LifecycleSummary::success("loop")
                            .branch(current_branch.clone())
                            .next_action("stop"),
                        None,
                    ));
                    break;
                }
                Err(e) => {
                    crate::logger::log(
                        loop_logger.as_ref(),
                        &format!("Decider error (continuing anyway): {e}"),
                    );
                    emit_event(
                        &opts.event_sink,
                        MrmouthEvent::failure("decider error", None, Some(e.to_string())),
                    );
                }
            }

            // Check if TUI user cancelled after decider
            if tui.is_some_and(|t| t.is_cancelled()) {
                emit(&tui_tx, "LOOP CANCELLED BY USER");
                terminal = Some(LoopTerminal::cancelled(&current_branch));
                break;
            }

            // --- Runner ---
            let run_opts = RunOptions {
                raw: false,
                json_events: opts.json_events,
                emit_terminal_events: false,
                model: opts.model.clone(),
                timeout: None,
                local: false,
                current_container: false,
                local_workspace_path: None,
                worktree_path: repo_layout.docker_work_mount(),
                repo_layout: Some(repo_layout.clone()),
                prompt_override: None,
                branch: Some(current_branch.clone()),
                event_sink: opts.event_sink.clone(),
            };

            let head_before = git_head(&repo_layout.work_repo);

            emit_event(
                &opts.event_sink,
                MrmouthEvent::RunLabel {
                    name: "run".to_string(),
                    value: format!("Run {run_number}"),
                },
            );
            let run_result = run::execute_in_session(config, repo_root, run_opts, &session, tui);
            let run_logger: Option<Logger> = match run_result {
                Ok(logger) => Some(logger),
                Err(e) => {
                    emit(&tui_tx, &format!("Run {run_number} failed: {e}"));
                    emit_event(
                        &opts.event_sink,
                        MrmouthEvent::failure(
                            format!("Run {run_number} failed"),
                            e.exit_code(),
                            Some(e.short_reason()),
                        ),
                    );
                    None
                }
            };
            // Use the run's logger if available, otherwise fall back to the loop logger.
            let logger_opt: Option<&Logger> = run_logger.as_ref().or(loop_logger.as_ref());

            // Check if TUI user cancelled during the run
            if tui.is_some_and(|t| t.is_cancelled()) {
                emit(&tui_tx, "LOOP CANCELLED BY USER");
                terminal = Some(LoopTerminal::cancelled(&current_branch));
                break;
            }

            // Sync litebrite so reviewer sees fresh task state
            emit_event(
                &opts.event_sink,
                MrmouthEvent::Sync {
                    action: SyncAction::Starting,
                    tool: SyncTool::Litebrite,
                    detail: Some("before reviewer".to_string()),
                },
            );
            litebrite::sync(repo_root, logger_opt);
            emit_event(
                &opts.event_sink,
                MrmouthEvent::Sync {
                    action: SyncAction::Finished,
                    tool: SyncTool::Litebrite,
                    detail: Some("before reviewer".to_string()),
                },
            );

            // --- Reviewer (only if the agent actually committed something) ---
            let head_after = git_head(&repo_layout.work_repo);
            let commit_range = match (&head_before, &head_after) {
                (Ok(before), Ok(after)) if before != after => Some((before.clone(), after.clone())),
                _ => None,
            };

            if commit_range.is_some() {
                emit_event(
                    &opts.event_sink,
                    MrmouthEvent::StageChanged {
                        stage: "Reviewer".to_string(),
                    },
                );
                let reviewer_opts = reviewer::ReviewerOptions {
                    model: config.effective_model_for_agent(&config.loop_config.reviewer_model),
                    current_branch: current_branch.clone(),
                    commit_range,
                    review_target: None,
                    worktree_path: repo_layout.docker_work_mount(),
                    event_sink: opts.event_sink.clone(),
                };
                let role_start = std::time::Instant::now();
                let reviewer_result =
                    reviewer::execute(config, repo_root, &reviewer_opts, logger_opt);
                crate::logger::log_timing(logger_opt, "reviewer-wall", role_start.elapsed());
                if let Err(e) = reviewer_result {
                    crate::logger::log(logger_opt, &format!("Reviewer failed (non-fatal): {e}"));
                    emit_event(
                        &opts.event_sink,
                        MrmouthEvent::failure("reviewer failed", None, Some(e.to_string())),
                    );
                }
                // Sync lb state pushed by reviewer container back to host
                emit_event(
                    &opts.event_sink,
                    MrmouthEvent::Sync {
                        action: SyncAction::Starting,
                        tool: SyncTool::Litebrite,
                        detail: Some("after reviewer".to_string()),
                    },
                );
                litebrite::sync(repo_root, logger_opt);
                emit_event(
                    &opts.event_sink,
                    MrmouthEvent::Sync {
                        action: SyncAction::Finished,
                        tool: SyncTool::Litebrite,
                        detail: Some("after reviewer".to_string()),
                    },
                );
            } else {
                crate::logger::log(
                    logger_opt,
                    "Reviewer skipped: no new commits from this run.",
                );
                emit_event(
                    &opts.event_sink,
                    MrmouthEvent::ReviewerLifecycle {
                        action: ReviewerAction::Skipped,
                        branch: current_branch.clone(),
                        commit_range: None,
                    },
                );
            }

            // --- Summary (runs after reviewer) ---
            if !opts.no_summary {
                let log_file = format!("{}/latest.jsonl", config.log_dir);
                let role_start = std::time::Instant::now();
                let summary_result = summary::execute(config, repo_root, &log_file, logger_opt);
                crate::logger::log_timing(logger_opt, "summary-wall", role_start.elapsed());
                if let Err(e) = summary_result {
                    crate::logger::log(logger_opt, &format!("Summary generation failed: {e}"));
                }
            }

            // Dockerfile-hash check: if the agent or decider edited the Dockerfile,
            // the session's image is now stale. Rebuild (cheap on cache hit) and
            // restart the session only if the image ID moved.
            if let Err(e) = maybe_restart_session_on_dockerfile_change(
                config,
                repo_root,
                &current_branch,
                &repo_layout,
                &mut session,
                tui,
                &tui_tx,
                session_logger,
            ) {
                emit(&tui_tx, &format!("Session restart failed: {e}"));
                return Err(LoopError::SessionStart(Box::new(e)));
            }

            // Check if TUI user cancelled before sleeping
            if tui.is_some_and(|t| t.is_cancelled()) {
                emit(&tui_tx, "LOOP CANCELLED BY USER");
                terminal = Some(LoopTerminal::cancelled(&current_branch));
                break;
            }

            if opts.delay > 0 {
                emit(
                    &tui_tx,
                    &format!("Waiting {}s until next run...", opts.delay),
                );
                std::thread::sleep(std::time::Duration::from_secs(opts.delay as u64));
            }
        }
        Ok(())
    })();

    // Tear down the session regardless of how the loop exited.
    run::stop_session(session, loop_logger.as_ref());

    if loop_result.is_ok() {
        if let Some(terminal) = terminal {
            emit_loop_terminal(
                &opts.event_sink,
                terminal.with_latest_log_paths(repo_root, &config.log_dir),
                loop_logger.as_ref(),
            );
        }
    }

    loop_result
}

fn emit_event(sink: &Option<EventSinkHandle>, event: MrmouthEvent) {
    if let Some(sink) = sink {
        sink.emit(event);
    }
}

impl LoopTerminal {
    fn with_latest_log_paths(mut self, repo_root: &Path, log_dir: &str) -> Self {
        self.summary = attach_latest_log_paths(repo_root, log_dir, self.summary);
        self
    }
}

fn attach_latest_log_paths(
    repo_root: &Path,
    log_dir: &str,
    mut summary: LifecycleSummary,
) -> LifecycleSummary {
    let log_path = repo_root.join(log_dir).join("latest.log");
    if log_path.exists() {
        summary = summary.log_path(log_path.display().to_string());
    }
    let jsonl_path = repo_root.join(log_dir).join("latest.jsonl");
    if jsonl_path.exists() {
        summary = summary.jsonl_path(jsonl_path.display().to_string());
    }
    summary
}

fn emit_loop_terminal(
    sink: &Option<EventSinkHandle>,
    terminal: LoopTerminal,
    logger: Option<&Logger>,
) {
    if let Some(logger) = logger {
        logger.flush();
    }
    if let Some((status, summary)) = terminal.finished {
        emit_event(sink, MrmouthEvent::finished(status, summary));
    }
    emit_event(
        sink,
        MrmouthEvent::LifecycleSummary {
            summary: terminal.summary,
        },
    );
}

/// Rebuild the Docker image (cheap if cached) and, if the new image ID
/// differs from the session's, tear down `*session` and replace it with a
/// fresh one. A build failure is non-fatal — we keep the existing session.
#[allow(clippy::too_many_arguments)]
fn maybe_restart_session_on_dockerfile_change(
    config: &Config,
    repo_root: &Path,
    current_branch: &str,
    repo_layout: &RepoLayout,
    session: &mut crate::run::Session,
    tui: Option<&TuiHandle>,
    tui_tx: &Option<TuiSender>,
    session_logger: &Logger,
) -> Result<(), crate::run::RunError> {
    let docker = crate::docker::DockerBuilder::new(&config.image);
    if docker.build(repo_root, &config.dockerfile).is_err() {
        return Ok(());
    }
    let new_image_id = docker.image_id().unwrap_or_default();
    if new_image_id.is_empty() || new_image_id == session.image_id {
        return Ok(());
    }

    emit(
        tui_tx,
        "Dockerfile changed — restarting session with new image...",
    );
    let session_log_path = repo_root.join(&config.log_dir).join("session.log");
    let fresh = crate::run::start_session(
        config,
        repo_root,
        current_branch,
        false,
        None,
        repo_layout.docker_work_mount(),
        tui,
        session_logger,
        &session_log_path,
    )?;
    let stale = std::mem::replace(session, fresh);
    crate::run::stop_session(stale, Some(session_logger));
    Ok(())
}

enum Decision {
    Continue(String),
    Ship(String),
    Stop(String),
}

fn should_continue(
    config: &Config,
    repo_root: &Path,
    decider_model: &str,
    logger: Option<&Logger>,
    log_dir: &Path,
) -> Result<Decision, LoopError> {
    crate::logger::log(logger, "DECISION");

    // Short-circuit: if open tasks already exist in the tracker, the runner has
    // work to do and we don't need to spend an LLM call on the decision. The
    // decider only earns its keep when there's actual judgement required —
    // decompose epics, check spec, decide ship/stop.
    if let Some(n) = crate::litebrite::open_task_count(repo_root) {
        if n > 0 {
            let msg = format!("open tasks: {n}");
            crate::logger::log(logger, &format!("decision: continue ({msg}) — skipped LLM"));
            return Ok(Decision::Continue(msg));
        }
    }

    // Create dedicated decider log + jsonl files
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let decider_log_path = log_dir.join(format!("decider-{timestamp}.log"));
    let decider_jsonl_path = log_dir.join(format!("decider-{timestamp}.jsonl"));

    let decider_logger = match logger.and_then(|l| l.display_sink()) {
        Some(display) => Logger::with_display_handle(&decider_log_path, display),
        None => Logger::new(&decider_log_path),
    }
    .ok();

    let mut jsonl_writer: Option<BufWriter<File>> =
        File::create(&decider_jsonl_path).ok().map(BufWriter::new);

    let schema = r#"{"type":"object","properties":{"action":{"type":"string","enum":["continue","ship","stop"],"description":"continue = more work to do, ship = all done and ready to merge, stop = nothing to merge"},"reason":{"type":"string","description":"Brief explanation of the decision"}},"required":["action","reason"]}"#;

    let prompt = format!("## System\n\n{}\n\n\
        You are the **Decider**. Your job is to assess project state and return a decision.\n\n\
        ## Boundary\n\n\
        You do NOT implement features, claim tasks, or make code changes. \
        You MAY create litebrite items, edit the Dockerfile, and read any file.\n\n\
        ## Instructions\n\n\
        1. Run `lb list` to check for open litebrite items.\n\
        2. Check whether the open items include **leaf tasks** (type=task with no children) that the runner can implement.\n\
           - Run `lb list --tree` to see the hierarchy.\n\
           - If leaf tasks exist, return **continue**.\n\
           - If the only open items are **epics or features with no child tasks**, decompose them: \
             create concrete child tasks with `lb create \"<title>\" -t task --parent <epic-id> -d \"<description>\"`. \
             Also read `.mrmouth/Dockerfile` and SPEC.md — if the spec requires a toolchain \
             (e.g. Rust, Go, Python) that is not installed in the Dockerfile, add the necessary \
             `RUN` commands **before** the `USER runner` line so the runner has a working compiler/interpreter. \
             Then return **continue**.\n\
        3. If NO open items exist, read SPEC.md and compare it against the current implementation.\n\
           - If there are deficiencies or missing features, create litebrite tasks for them \
             (and optionally edit `.mrmouth/Dockerfile` if tooling changes are needed), then return **continue**.\n\
           - If the implementation fully satisfies the spec, close any parent epics/features \
             whose children are all closed (`lb close <id>`), then check if the current branch has commits \
             ahead of main: `git rev-list --count HEAD --not main`. If > 0, return **ship**. \
             If 0, return **stop** (nothing to merge).\n\n\
        **Important:** If you edit any files (e.g. `.mrmouth/Dockerfile`), you MUST commit and push \
        before returning your decision: `git add -A && git commit -m \"<message>\" && git push`.\n\n\
        **Ship** means: all litebrite items are closed and the implementation matches the spec. \
        It merges the current branch and stops.\n\n\
        Actions:\n\
        - \"continue\": there is work remaining for the runner\n\
        - \"ship\": all work is complete — merge the branch\n\
        - \"stop\": nothing was done, nothing to merge",
        crate::prompt::SYSTEM_PREAMBLE);

    let mut cmd = streaming::agent_stream_cmd_with_schema(
        config.agent,
        repo_root,
        decider_model,
        "Read,Edit,Write,Bash(git *),Bash(lb *)",
        schema,
    );

    let mut child = cmd.spawn().map_err(|e| {
        LoopError::Decider(format!("failed to run {} CLI: {e}", config.agent.as_str()))
    })?;

    streaming::send_prompt(&mut child, &prompt);

    let target = match logger.and_then(|l| l.display_sink()) {
        Some(display) => StreamTarget::Display(display),
        None => StreamTarget::Stderr,
    };

    let mut formatter = StreamFormatter::new(target.supports_color());

    let effective_logger = decider_logger.as_ref().or(logger);
    let (result_text, exit_code) = streaming::run_streaming_claude(
        child,
        &mut formatter,
        effective_logger,
        &target,
        &mut jsonl_writer,
    )
    .map_err(|e| LoopError::Decider(format!("streaming error: {e}")))?;

    if exit_code != 0 {
        return Err(LoopError::Decider(format!(
            "{} CLI exited with code {exit_code}",
            config.agent.as_str()
        )));
    }

    // Parse the structured result from the stream-json result event
    let parsed: serde_json::Value = match serde_json::from_str(&result_text) {
        Ok(v) => v,
        Err(e) => {
            crate::logger::log(
                effective_logger,
                &format!("WARNING: decider returned invalid JSON (defaulting to 'continue'): {e}"),
            );
            crate::logger::log(effective_logger, &format!("  raw output: {result_text}"));
            return Ok(Decision::Continue(
                "JSON parse failure — defaulting to continue".into(),
            ));
        }
    };
    let action = parsed["action"].as_str().unwrap_or("continue");
    let reason = parsed["reason"]
        .as_str()
        .unwrap_or("no reason given")
        .to_string();

    match action {
        "ship" => Ok(Decision::Ship(reason)),
        "stop" => Ok(Decision::Stop(reason)),
        _ => Ok(Decision::Continue(reason)),
    }
}

fn git_head(repo_root: &Path) -> Result<String, ()> {
    let output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "rev-parse", "HEAD"])
        .output()
        .map_err(|_| ())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(())
    }
}

fn git_current_branch(repo_root: &Path) -> Result<String, LoopError> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "branch",
            "--show-current",
        ])
        .output()
        .map_err(|e| LoopError::BranchCreation(format!("failed to get current branch: {e}")))?;

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        Ok("main".into())
    } else {
        Ok(branch)
    }
}

#[derive(Debug)]
pub enum LoopError {
    Bootstrap(String),
    Decider(String),
    BranchCreation(String),
    SessionStart(Box<crate::run::RunError>),
}

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bootstrap(msg) => write!(f, "bootstrap error: {msg}"),
            Self::Decider(msg) => write!(f, "decider error: {msg}"),
            Self::BranchCreation(msg) => write!(f, "branch creation error: {msg}"),
            Self::SessionStart(e) => write!(f, "session start error: {e}"),
        }
    }
}

impl std::error::Error for LoopError {}

impl LoopError {
    /// Most LoopError variants fail before a run log exists, so the debrief
    /// just carries the Display message. SessionStart delegates to the inner
    /// RunError so the session-setup log path + tail are surfaced.
    pub fn debrief(&self) -> crate::debrief::FailureDebrief {
        match self {
            Self::SessionStart(e) => e.debrief(),
            _ => crate::debrief::FailureDebrief::new(self.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::RecordingEventSink;

    #[test]
    fn attach_latest_log_paths_adds_existing_latest_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        std::fs::write(dir.path().join("logs/latest.log"), "log\n").unwrap();
        std::fs::write(dir.path().join("logs/latest.jsonl"), "{}\n").unwrap();

        let summary =
            attach_latest_log_paths(dir.path(), "logs", LifecycleSummary::success("loop"));

        assert_eq!(
            summary.log_path.as_deref(),
            Some(dir.path().join("logs/latest.log").display().to_string()).as_deref()
        );
        assert_eq!(
            summary.jsonl_path.as_deref(),
            Some(dir.path().join("logs/latest.jsonl").display().to_string()).as_deref()
        );
    }

    #[test]
    fn emit_loop_terminal_flushes_log_before_lifecycle_summary() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("loop.log");
        let logger = Logger::new(&log_path).unwrap();
        logger.log_file_only("late summary marker");

        let recording = RecordingEventSink::default();
        let sink = Some(EventSinkHandle::new(recording.clone()));
        let terminal = LoopTerminal::success(
            LifecycleSummary::success("loop").branch("feature"),
            Some("max runs reached"),
        );

        emit_loop_terminal(&sink, terminal, Some(&logger));

        let log = std::fs::read_to_string(&log_path).unwrap();
        assert!(log.contains("late summary marker"));

        let events = recording.events();
        assert!(matches!(
            events.as_slice(),
            [
                MrmouthEvent::Finished {
                    status: FinishStatus::Success,
                    summary: Some(_),
                },
                MrmouthEvent::LifecycleSummary { .. },
            ]
        ));
    }
}
