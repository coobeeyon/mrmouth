use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::agent::AgentKind;
use crate::config::Config;
use crate::docker::{ContainerArgs, CopyFromContainerOutcome, DockerBuilder};
use crate::events::{
    ContainerAction, EventSink, EventSinkHandle, FinishStatus, LifecycleSummary, MessageLevel,
    MessageTarget, MrmouthEvent, RunAction, SyncAction, SyncTool,
};
use crate::litebrite;
use crate::logger::Logger;
use crate::prompt;
use crate::repo_layout::RepoLayout;
use crate::stream_fmt::{self, StreamFormatter};
use crate::streaming::{self, StreamTarget};
use crate::tui::TuiHandle;

/// Guard that sets an AtomicBool to true on drop, ensuring the cancel watcher
/// thread is signaled even if the function returns early via `?`.
struct DoneGuard(Arc<AtomicBool>);

impl Drop for DoneGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub struct RunOptions {
    pub raw: bool,
    pub json_events: bool,
    pub model: String,
    pub timeout: Option<u32>,
    pub local: bool,
    pub current_container: bool,
    pub local_workspace_path: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub repo_layout: Option<RepoLayout>,
    pub prompt_override: Option<String>,
    pub branch: Option<String>,
    pub event_sink: Option<EventSinkHandle>,
}

/// A long-lived session container shared across multiple task runs (epic mode).
/// The caller (typically `do_cmd::execute_epic`) owns a Session and feeds it to
/// `execute_in_session` per task. `start_session` builds the image and boots the
/// container; `stop_session` tears it down.
pub struct Session {
    pub container_name: String,
    pub image_id: String,
    pub scripts_dir: tempfile::TempDir,
    pub local: bool,
    pub worktree_path: Option<PathBuf>,
    pub file_remote_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamDisplayMode {
    Raw,
    LifecycleJson,
    Formatted,
}

fn stream_display_mode(opts: &RunOptions) -> StreamDisplayMode {
    if opts.raw {
        StreamDisplayMode::Raw
    } else if opts.json_events {
        StreamDisplayMode::LifecycleJson
    } else {
        StreamDisplayMode::Formatted
    }
}

struct RunReporter<'a> {
    sink: Option<&'a EventSinkHandle>,
    tui_sink: Option<crate::tui::TuiEventSink>,
    display_via_events: bool,
}

impl<'a> RunReporter<'a> {
    fn new(sink: Option<&'a EventSinkHandle>, tui: Option<&'a TuiHandle>) -> Self {
        Self {
            sink,
            tui_sink: sink
                .is_none()
                .then(|| tui.map(TuiHandle::event_sink))
                .flatten(),
            display_via_events: tui.is_some(),
        }
    }

    fn emit(&self, event: MrmouthEvent) {
        if let Some(tui_sink) = &self.tui_sink {
            tui_sink.emit(&event);
        }
        if let Some(sink) = self.sink {
            sink.emit(event);
        }
    }

    fn log(&self, logger: &Logger, msg: &str) {
        self.emit(MrmouthEvent::Message {
            level: MessageLevel::Info,
            text: msg.to_string(),
            target: MessageTarget::Agent,
        });
        if self.display_via_events {
            logger.log_file_only(msg);
        } else {
            logger.log(msg);
        }
    }
}

fn log_status(logger: &Logger, reporter: Option<&RunReporter<'_>>, msg: &str) {
    match reporter {
        Some(reporter) => reporter.log(logger, msg),
        None => logger.log(msg),
    }
}

fn pull_code_changes(repo_root: &Path, logger: &Logger, reporter: Option<&RunReporter<'_>>) {
    log_status(logger, reporter, "Pulling code changes from remote...");
    let pull_output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "pull", "--ff-only"])
        .output();
    match pull_output {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            if stderr.contains("Already up to date") || stderr.is_empty() {
                log_status(logger, reporter, "No new commits to pull.");
            } else {
                log_status(
                    logger,
                    reporter,
                    &format!("Warning: git pull failed: {}", stderr.trim()),
                );
            }
        }
        Err(e) => log_status(logger, reporter, &format!("Warning: git pull failed: {e}")),
    }
}

fn extract_updated_dockerfile(
    repo_root: &Path,
    dockerfile_path: &str,
    container_name: &str,
    logger: &Logger,
    reporter: Option<&RunReporter<'_>>,
) {
    let dockerfile_dest = repo_root.join(dockerfile_path);
    let container_path = format!("/home/runner/workspace/{dockerfile_path}");
    match DockerBuilder::copy_from_container_if_changed(
        container_name,
        &container_path,
        &dockerfile_dest,
    ) {
        Ok(CopyFromContainerOutcome::Updated) => {
            log_status(
                logger,
                reporter,
                "Extracted updated Dockerfile from container.",
            );
        }
        Ok(CopyFromContainerOutcome::Unchanged) => {
            log_status(
                logger,
                reporter,
                "Dockerfile from container matches host; leaving worktree unchanged.",
            );
        }
        Ok(CopyFromContainerOutcome::Missing) => {}
        Err(e) => log_status(
            logger,
            reporter,
            &format!("Warning: Dockerfile extraction failed: {e}"),
        ),
    }
}

/// Execute one agent run. Returns the Logger so callers can continue writing to the same
/// log file for subsequent stages (reviewer, decider, summary, etc.).
pub fn execute(
    config: &Config,
    repo_root: &Path,
    opts: RunOptions,
    tui: Option<&TuiHandle>,
) -> Result<Logger, RunError> {
    if opts.current_container && opts.worktree_path.is_none() {
        return Err(RunError::Preflight(
            "current-container mode requires a distinct work repo via --worktree <path> or work_repo in .mrmouth/config.toml"
                .into(),
        ));
    }

    let reporter = RunReporter::new(opts.event_sink.as_ref(), tui);
    reporter.emit(MrmouthEvent::StageChanged {
        stage: "Agent".to_string(),
    });
    reporter.emit(MrmouthEvent::RunLifecycle {
        action: RunAction::Starting,
        run_id: None,
        branch: None,
    });
    // 0. Set up logging first so every stage is captured
    let log_dir = repo_root.join(&config.log_dir);
    fs::create_dir_all(&log_dir).map_err(|e| RunError::Io("creating log directory".into(), e))?;
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let log_filename = format!("run-{timestamp}.log");
    let log_path = log_dir.join(&log_filename);
    let logger = match tui {
        Some(t) => Logger::with_display_sink(&log_path, t.sender("AGENT SESSION"))
            .map_err(|e| RunError::Io("creating log file".into(), e))?,
        None => Logger::new(&log_path).map_err(|e| RunError::Io("creating log file".into(), e))?,
    };

    // Resolve branch early so we can include it in the opening banner
    let branch = opts
        .branch
        .clone()
        .or_else(|| config.branch.clone())
        .unwrap_or_else(|| git_current_branch(repo_root).unwrap_or_else(|_| "main".into()));

    reporter.log(&logger, &format!("AGENT RUN  branch={branch}  {timestamp}"));
    reporter.emit(MrmouthEvent::RunLabel {
        name: "branch".to_string(),
        value: branch.clone(),
    });
    if opts.current_container {
        reporter.emit(MrmouthEvent::RunLabel {
            name: "workspace".to_string(),
            value: opts
                .worktree_path
                .as_deref()
                .unwrap_or(repo_root)
                .display()
                .to_string(),
        });
    } else if let Some(path) = opts.worktree_path.as_ref() {
        reporter.emit(MrmouthEvent::RunLabel {
            name: "workspace".to_string(),
            value: path.display().to_string(),
        });
    }
    if opts.current_container {
        reporter.emit(MrmouthEvent::RunLabel {
            name: "mode".to_string(),
            value: "current_container".to_string(),
        });
        return execute_current_container(
            config,
            repo_root,
            &opts,
            tui,
            logger,
            log_dir,
            timestamp,
            log_filename,
            log_path,
            branch,
            &reporter,
        );
    }

    // 1. Preflight checks
    let has_local_only_tooling = has_local_only_tooling_branch(repo_root);
    let local = opts.local || has_local_only_tooling;
    if has_local_only_tooling && !opts.local {
        reporter.log(
            &logger,
            "Tooling branch exists locally but not on remote — using local mode.",
        );
    }
    let effective_dockerfile =
        crate::docker::effective_dockerfile_content(repo_root, &config.dockerfile);
    if preflight_skipped() {
        reporter.log(
            &logger,
            "MRMOUTH_SKIP_PREFLIGHT=1 — skipping preflight checks.",
        );
    } else {
        reporter.log(&logger, "Checking preflight conditions...");
    }
    reporter.emit(MrmouthEvent::RunLifecycle {
        action: RunAction::Preflight,
        run_id: Some(log_filename.clone()),
        branch: Some(branch.clone()),
    });
    preflight(
        repo_root,
        config.agent,
        local,
        false,
        &effective_dockerfile,
        Some(&logger),
    )
    .inspect_err(|_| {
        logger.flush();
    })?;

    // 2. Resolve repo URL
    let (repo_url, file_remote_path) = if local {
        (String::new(), None)
    } else {
        match git_remote_url(repo_root) {
            Some(url) => (url, None),
            None => {
                configure_file_remote(repo_root)?;
                (
                    "file:///host-repo".to_string(),
                    Some(repo_root.to_path_buf()),
                )
            }
        }
    };

    // 3. Sync litebrite (best-effort)
    reporter.log(&logger, "Syncing litebrite...");
    reporter.emit(MrmouthEvent::Sync {
        action: SyncAction::Starting,
        tool: SyncTool::Litebrite,
        detail: None,
    });
    litebrite::init_and_sync(repo_root, Some(&logger));
    reporter.emit(MrmouthEvent::Sync {
        action: SyncAction::Finished,
        tool: SyncTool::Litebrite,
        detail: None,
    });

    // 4. Write runner entrypoint script
    let prompt_override = opts
        .prompt_override
        .clone()
        .or_else(|| default_prompt_override(repo_root, &opts, Some(&logger)));
    let runner_script = write_runner_script(
        config.agent,
        repo_root,
        &opts.model,
        prompt_override.as_deref(),
        &effective_dockerfile,
        &config.dockerfile,
        Some(&logger),
    )?;

    // 5. Build Docker image
    reporter.log(&logger, "Docker build starting...");
    reporter.emit(MrmouthEvent::RunLifecycle {
        action: RunAction::BuildingImage,
        run_id: Some(log_filename.clone()),
        branch: Some(branch.clone()),
    });
    reporter.emit(MrmouthEvent::ContainerLifecycle {
        action: ContainerAction::BuildingImage,
        name: config.image.clone(),
        image_id: None,
        exit_code: None,
    });
    let docker = DockerBuilder::new(&config.image);
    let build_start = std::time::Instant::now();
    docker
        .build(repo_root, &config.dockerfile)
        .map_err(RunError::Docker)?;
    reporter.log(
        &logger,
        &format!(
            "::mrmouth::timing phase=docker-build elapsed_ms={}",
            build_start.elapsed().as_millis()
        ),
    );
    reporter.emit(MrmouthEvent::ContainerLifecycle {
        action: ContainerAction::ImageBuilt,
        name: config.image.clone(),
        image_id: docker.image_id(),
        exit_code: None,
    });

    // 6. Ensure persistent volume
    let volume = config.effective_volume(repo_root);
    docker.ensure_volume(&volume).map_err(RunError::Docker)?;

    // 7. Set up JSONL log alongside the text log
    let jsonl_filename = format!("run-{timestamp}.jsonl");
    let jsonl_path = log_dir.join(&jsonl_filename);

    let container_name = format!("run-{timestamp}");

    // Remove stale container
    DockerBuilder::remove_container(&container_name);

    // 8. Start container
    reporter.log(
        &logger,
        &format!("AGENT SESSION  container={container_name}"),
    );
    reporter.log(&logger, &format!("Branch: {branch}"));
    reporter.emit(MrmouthEvent::ContainerLifecycle {
        action: ContainerAction::Starting,
        name: container_name.clone(),
        image_id: docker.image_id(),
        exit_code: None,
    });

    let container_args = container_args_from_run_options(
        container_name.clone(),
        repo_url,
        branch.clone(),
        runner_script.to_path_buf(),
        volume,
        config.agent.home_mount(),
        local,
        file_remote_path.clone(),
        &opts,
    );

    let container_start = std::time::Instant::now();
    let mut handle = docker.run(&container_args).map_err(RunError::Docker)?;
    reporter.emit(MrmouthEvent::ContainerLifecycle {
        action: ContainerAction::Started,
        name: container_name.clone(),
        image_id: docker.image_id(),
        exit_code: None,
    });

    // 8b. Spawn a watcher that stops the container if the TUI user cancels (q / Ctrl+C)
    let watcher_done = Arc::new(AtomicBool::new(false));
    // DoneGuard ensures watcher_done is set even if we return early via `?`,
    // preventing the cancel watcher thread from spinning indefinitely.
    let _done_guard = DoneGuard(Arc::clone(&watcher_done));
    let _cancel_watcher = if let Some(t) = tui {
        let flag = t.cancelled_flag();
        let done = Arc::clone(&watcher_done);
        let name = container_name.clone();
        Some(std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) && !done.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            if flag.load(Ordering::Relaxed) {
                DockerBuilder::stop_container(&name);
            }
        }))
    } else {
        None
    };

    // 9. Stream output — raw JSONL to .jsonl file, formatted text to terminal + .log file
    let jsonl_file =
        File::create(&jsonl_path).map_err(|e| RunError::Io("creating jsonl file".into(), e))?;
    let mut jsonl_writer = BufWriter::new(jsonl_file);
    let is_tty = logger.display_supports_color() || std::io::stdout().is_terminal();

    match stream_display_mode(&opts) {
        StreamDisplayMode::Raw => {
            let stdout = std::io::stdout();
            handle
                .stream_output(|line| {
                    if should_suppress_stream_line(config.agent, line) {
                        return;
                    }
                    let _ = writeln!(stdout.lock(), "{line}");
                    let _ = writeln!(jsonl_writer, "{line}");
                    logger.log_file_only(line);
                })
                .map_err(RunError::Docker)?;
        }
        StreamDisplayMode::LifecycleJson => {
            handle
                .stream_output(|line| {
                    if should_suppress_stream_line(config.agent, line) {
                        return;
                    }
                    let _ = writeln!(jsonl_writer, "{line}");
                    logger.log_file_only(line);
                })
                .map_err(RunError::Docker)?;
        }
        StreamDisplayMode::Formatted => {
            let mut formatter = StreamFormatter::new(is_tty);
            handle
                .stream_output(|line| {
                    if should_suppress_stream_line(config.agent, line) {
                        return;
                    }
                    let _ = writeln!(jsonl_writer, "{line}");
                    if let Some(formatted) = stream_fmt::format_line(&mut formatter, line) {
                        logger.display(&formatted);
                        logger.log_file_only(&formatted);
                    }
                })
                .map_err(RunError::Docker)?;
        }
    }

    let _ = jsonl_writer.flush();

    // 10. Wait for container exit
    let exit_code = handle.wait().map_err(RunError::Docker)?;
    watcher_done.store(true, Ordering::Relaxed);
    reporter.log(
        &logger,
        &format!(
            "::mrmouth::timing phase=container-wall elapsed_ms={}",
            container_start.elapsed().as_millis()
        ),
    );
    reporter.log(
        &logger,
        &format!("Container {container_name} finished (exit code {exit_code})."),
    );
    reporter.emit(MrmouthEvent::ContainerLifecycle {
        action: ContainerAction::Exited,
        name: container_name.clone(),
        image_id: docker.image_id(),
        exit_code: Some(exit_code),
    });

    // 11. Update symlinks atomically (latest.jsonl and latest.log)
    let latest_jsonl = log_dir.join("latest.jsonl");
    let latest_log = log_dir.join("latest.log");
    #[cfg(unix)]
    {
        atomic_symlink(&jsonl_filename, &latest_jsonl);
        atomic_symlink(&log_filename, &latest_log);
    }

    // 12. Post-run sync before extracting files. This keeps self-produced,
    // already-pushed Dockerfile edits from dirtying the host before pull.
    reporter.log(&logger, "Post-run sync...");
    reporter.emit(MrmouthEvent::RunLifecycle {
        action: RunAction::PullingChanges,
        run_id: Some(log_filename.clone()),
        branch: Some(branch.clone()),
    });
    if !local && file_remote_path.is_none() {
        pull_code_changes(repo_root, &logger, Some(&reporter));
    }

    // 13. Extract updated Dockerfile from container (agent may have modified it)
    if !local {
        reporter.emit(MrmouthEvent::RunLifecycle {
            action: RunAction::ExtractingDockerfile,
            run_id: Some(log_filename.clone()),
            branch: Some(branch.clone()),
        });
        extract_updated_dockerfile(
            repo_root,
            &config.dockerfile,
            &container_name,
            &logger,
            Some(&reporter),
        );
    }

    // 14. Clean up container
    reporter.emit(MrmouthEvent::ContainerLifecycle {
        action: ContainerAction::Removed,
        name: container_name.clone(),
        image_id: docker.image_id(),
        exit_code: None,
    });
    DockerBuilder::remove_container(&container_name);

    reporter.emit(MrmouthEvent::Sync {
        action: SyncAction::Starting,
        tool: SyncTool::Litebrite,
        detail: Some("post-run".to_string()),
    });
    litebrite::init_and_sync(repo_root, Some(&logger));
    reporter.emit(MrmouthEvent::Sync {
        action: SyncAction::Finished,
        tool: SyncTool::Litebrite,
        detail: Some("post-run".to_string()),
    });
    reporter.log(&logger, &format!("Done. Log saved: {}", log_path.display()));

    if exit_code != 0 {
        // Flush so classify_exit reads everything the runner script wrote.
        logger.flush();
        let reason = classify_exit(exit_code, &log_path);
        reporter.emit(MrmouthEvent::LifecycleSummary {
            summary: LifecycleSummary::failed(
                "run",
                format!("container exited with code {exit_code}: {reason}"),
            )
            .branch(branch.clone())
            .workspace(run_workspace_label(local, opts.worktree_path.as_deref()))
            .log_path(log_path.display().to_string())
            .jsonl_path(jsonl_path.display().to_string())
            .exit_code(exit_code)
            .next_action("inspect_log"),
        });
        return Err(RunError::ContainerFailed {
            code: exit_code,
            reason,
            log_path: log_path.clone(),
        });
    }

    reporter.emit(MrmouthEvent::RunLifecycle {
        action: RunAction::Finished,
        run_id: Some(log_filename),
        branch: Some(branch.clone()),
    });
    reporter.emit(MrmouthEvent::finished(
        FinishStatus::Success,
        None::<String>,
    ));
    reporter.emit(MrmouthEvent::LifecycleSummary {
        summary: LifecycleSummary::success("run")
            .branch(branch)
            .workspace(run_workspace_label(local, opts.worktree_path.as_deref()))
            .log_path(log_path.display().to_string())
            .jsonl_path(jsonl_path.display().to_string())
            .exit_code(exit_code)
            .next_action("none"),
    });

    Ok(logger)
}

#[allow(clippy::too_many_arguments)]
fn execute_current_container(
    config: &Config,
    repo_root: &Path,
    opts: &RunOptions,
    tui: Option<&TuiHandle>,
    logger: Logger,
    log_dir: PathBuf,
    timestamp: String,
    log_filename: String,
    log_path: PathBuf,
    branch: String,
    reporter: &RunReporter<'_>,
) -> Result<Logger, RunError> {
    let current_workspace = opts
        .worktree_path
        .as_deref()
        .unwrap_or(repo_root)
        .display()
        .to_string();
    reporter.log(
        &logger,
        "Current-container mode: running agent CLI directly in the current checkout.",
    );
    reporter.emit(MrmouthEvent::RunLifecycle {
        action: RunAction::Preflight,
        run_id: Some(log_filename.clone()),
        branch: Some(branch.clone()),
    });
    if preflight_skipped() {
        reporter.log(
            &logger,
            "MRMOUTH_SKIP_PREFLIGHT=1 — skipping current-container preflight checks.",
        );
    } else {
        check_agent_credentials(config.agent)?;
        check_current_container_tools(repo_root, config.agent)?;
    }

    reporter.log(&logger, "Syncing litebrite...");
    reporter.emit(MrmouthEvent::Sync {
        action: SyncAction::Starting,
        tool: SyncTool::Litebrite,
        detail: None,
    });
    litebrite::init_and_sync(repo_root, Some(&logger));
    reporter.emit(MrmouthEvent::Sync {
        action: SyncAction::Finished,
        tool: SyncTool::Litebrite,
        detail: None,
    });

    let prompt_text = prompt_text_for_run(repo_root, opts, Some(&logger));

    let jsonl_filename = format!("run-{timestamp}.jsonl");
    let jsonl_path = log_dir.join(&jsonl_filename);
    let jsonl_file =
        File::create(&jsonl_path).map_err(|e| RunError::Io("creating jsonl file".into(), e))?;
    let mut jsonl_writer = Some(BufWriter::new(jsonl_file));

    reporter.emit(MrmouthEvent::RunLifecycle {
        action: RunAction::RunningAgent,
        run_id: Some(log_filename.clone()),
        branch: Some(branch.clone()),
    });
    reporter.log(&logger, "Starting current-container agent run...");
    let mut cmd = streaming::agent_runner_stream_cmd(config.agent, repo_root, &opts.model);
    cmd.env("MRMOUTH_TRACKING_REPO", repo_root);
    cmd.env("MRMOUTH_BOOKKEEPING_REPO", repo_root);
    if let Some(path) = opts.worktree_path.as_ref() {
        cmd.env("MRMOUTH_WORKTREE", path);
        cmd.env("MRMOUTH_WORK_REPO", path);
    } else {
        cmd.env("MRMOUTH_WORK_REPO", repo_root);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| RunError::Io(format!("spawning {} CLI", config.agent.as_str()), e))?;
    streaming::send_prompt(&mut child, &prompt_text);

    let watcher_done = Arc::new(AtomicBool::new(false));
    let _done_guard = DoneGuard(Arc::clone(&watcher_done));
    let _watcher = spawn_current_container_watcher(
        tui,
        opts.timeout.map(|m| m as u64 * 60),
        child.id(),
        Arc::clone(&watcher_done),
    );

    let run_start = std::time::Instant::now();
    let exit_code = if opts.raw {
        run_current_container_raw(child, config.agent, &logger, &mut jsonl_writer)
    } else {
        let target = match logger.display_sink() {
            Some(display) => StreamTarget::Display(display),
            None => StreamTarget::Stderr,
        };
        let mut formatter = StreamFormatter::new(target.supports_color());
        streaming::run_streaming_claude(
            child,
            &mut formatter,
            Some(&logger),
            &target,
            &mut jsonl_writer,
        )
        .map(|(_, exit_code)| exit_code)
    }
    .map_err(|e| RunError::Io("streaming current-container agent".into(), e))?;
    watcher_done.store(true, Ordering::Relaxed);

    reporter.log(
        &logger,
        &format!(
            "::mrmouth::timing phase=current-container-wall elapsed_ms={}",
            run_start.elapsed().as_millis()
        ),
    );
    reporter.log(
        &logger,
        &format!("Current-container agent finished (exit code {exit_code})."),
    );

    let latest_jsonl = log_dir.join("latest.jsonl");
    let latest_log = log_dir.join("latest.log");
    #[cfg(unix)]
    {
        atomic_symlink(&jsonl_filename, &latest_jsonl);
        atomic_symlink(&log_filename, &latest_log);
    }

    reporter.log(&logger, "Post-run sync...");
    reporter.emit(MrmouthEvent::Sync {
        action: SyncAction::Starting,
        tool: SyncTool::Litebrite,
        detail: Some("post-run".to_string()),
    });
    litebrite::init_and_sync(repo_root, Some(&logger));
    reporter.emit(MrmouthEvent::Sync {
        action: SyncAction::Finished,
        tool: SyncTool::Litebrite,
        detail: Some("post-run".to_string()),
    });
    logger.log(&format!("Done. Log saved: {}", log_path.display()));

    if exit_code != 0 {
        logger.flush();
        let reason = classify_exit(exit_code, &log_path);
        reporter.emit(MrmouthEvent::LifecycleSummary {
            summary: LifecycleSummary::failed(
                "run",
                format!("agent exited with code {exit_code}: {reason}"),
            )
            .branch(branch.clone())
            .workspace(current_workspace.clone())
            .log_path(log_path.display().to_string())
            .jsonl_path(jsonl_path.display().to_string())
            .exit_code(exit_code)
            .next_action("inspect_log"),
        });
        return Err(RunError::ProcessFailed {
            code: exit_code,
            reason,
            log_path,
        });
    }

    reporter.emit(MrmouthEvent::RunLifecycle {
        action: RunAction::Finished,
        run_id: Some(log_filename),
        branch: Some(branch.clone()),
    });
    reporter.emit(MrmouthEvent::finished(
        FinishStatus::Success,
        None::<String>,
    ));
    reporter.emit(MrmouthEvent::LifecycleSummary {
        summary: LifecycleSummary::success("run")
            .branch(branch)
            .workspace(current_workspace)
            .log_path(log_path.display().to_string())
            .jsonl_path(jsonl_path.display().to_string())
            .exit_code(exit_code)
            .next_action("none"),
    });

    Ok(logger)
}

fn run_current_container_raw(
    mut child: Child,
    agent: AgentKind,
    logger: &Logger,
    jsonl_writer: &mut Option<BufWriter<File>>,
) -> Result<i32, std::io::Error> {
    let tee_handle = child.stderr.take().map(|stderr| logger.tee_stderr(stderr));
    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        for line in reader.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() || should_suppress_stream_line(agent, trimmed) {
                continue;
            }
            let _ = writeln!(stdout, "{trimmed}");
            if let Some(writer) = jsonl_writer.as_mut() {
                let _ = writeln!(writer, "{trimmed}");
            }
            logger.log_file_only(trimmed);
        }
    }
    if let Some(writer) = jsonl_writer.as_mut() {
        let _ = writer.flush();
    }
    if let Some(handle) = tee_handle {
        let _ = handle.join();
    }
    let status = child.wait()?;
    Ok(status.code().unwrap_or(-1))
}

fn emit_event(sink: &Option<EventSinkHandle>, event: MrmouthEvent) {
    if let Some(sink) = sink {
        sink.emit(event);
    }
}

/// Boot a long-lived session container: preflight, build image, start detached,
/// run setup.sh once. Output is streamed into `logger` (typically the epic
/// logger). Call `stop_session` when done.
pub fn start_session(
    config: &Config,
    repo_root: &Path,
    initial_branch: &str,
    local: bool,
    local_workspace_path: Option<PathBuf>,
    worktree_path: Option<PathBuf>,
    tui: Option<&TuiHandle>,
    logger: &Logger,
    log_path: &Path,
) -> Result<Session, RunError> {
    // 1. Preflight
    let local = local || has_local_only_tooling_branch(repo_root);
    let effective_dockerfile =
        crate::docker::effective_dockerfile_content(repo_root, &config.dockerfile);
    if preflight_skipped() {
        logger.log("MRMOUTH_SKIP_PREFLIGHT=1 — skipping preflight checks.");
    } else {
        logger.log("Checking preflight conditions...");
    }
    preflight(
        repo_root,
        config.agent,
        local,
        false,
        &effective_dockerfile,
        Some(logger),
    )
    .inspect_err(|_| logger.flush())?;

    // 2. Repo URL
    let (repo_url, file_remote_path) = if local {
        (String::new(), None)
    } else {
        match git_remote_url(repo_root) {
            Some(url) => (url, None),
            None => {
                configure_file_remote(repo_root)?;
                (
                    "file:///host-repo".to_string(),
                    Some(repo_root.to_path_buf()),
                )
            }
        }
    };

    // 3. Sync litebrite (best-effort)
    logger.log("Syncing litebrite...");
    litebrite::init_and_sync(repo_root, Some(logger));

    // 4. Build image
    logger.log("Docker build starting...");
    let docker = DockerBuilder::new(&config.image);
    let build_start = std::time::Instant::now();
    docker
        .build(repo_root, &config.dockerfile)
        .map_err(RunError::Docker)?;
    logger.log(&format!(
        "::mrmouth::timing phase=docker-build elapsed_ms={}",
        build_start.elapsed().as_millis()
    ));
    let image_id = docker.image_id().unwrap_or_default();

    // 5. Ensure volume
    let volume = config.effective_volume(repo_root);
    docker.ensure_volume(&volume).map_err(RunError::Docker)?;

    // 6. Scripts dir + setup.sh
    let scripts_dir =
        tempfile::tempdir().map_err(|e| RunError::Io("creating scripts dir".into(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        let _ = std::fs::set_permissions(scripts_dir.path(), perms);
    }
    write_setup_script(
        config.agent,
        scripts_dir.path(),
        &effective_dockerfile,
        &config.dockerfile,
    )?;

    // 7. Start detached container
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let container_name = format!("session-{timestamp}");
    DockerBuilder::remove_container(&container_name);

    let session_args = session_args(
        container_name.clone(),
        repo_url.clone(),
        scripts_dir.path().to_path_buf(),
        volume.clone(),
        config.agent.home_mount(),
        local,
        local_workspace_path,
        worktree_path.clone(),
        file_remote_path.clone(),
    );
    docker
        .start_session(&session_args)
        .map_err(RunError::Docker)?;
    logger.log(&format!(
        "SESSION START  container={container_name}  image={image_id}"
    ));

    // 8. Run setup.sh (generous timeout — includes clone + deps)
    let env_vars = vec![
        ("REPO_URL".to_string(), repo_url),
        ("BRANCH".to_string(), initial_branch.to_string()),
    ];
    let setup_start = std::time::Instant::now();
    let mut handle = crate::docker::DockerBuilder::exec_script(
        &container_name,
        "setup.sh",
        &env_vars,
        Some(600),
    )
    .map_err(RunError::Docker)?;

    // Cancel watcher: if user hits quit during setup, stop the container so
    // setup.sh aborts instead of running to its 10-minute timeout.
    let watcher_done = Arc::new(AtomicBool::new(false));
    let _done_guard = DoneGuard(Arc::clone(&watcher_done));
    let _cancel_watcher = if let Some(t) = tui {
        let flag = t.cancelled_flag();
        let done = Arc::clone(&watcher_done);
        let name = container_name.clone();
        Some(std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) && !done.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            if flag.load(Ordering::Relaxed) {
                DockerBuilder::stop_container(&name);
            }
        }))
    } else {
        None
    };

    handle
        .stream_output(|line| logger.log(line))
        .map_err(RunError::Docker)?;
    let exit = handle.wait().map_err(RunError::Docker)?;
    watcher_done.store(true, Ordering::Relaxed);
    logger.log(&format!(
        "::mrmouth::timing phase=session-setup elapsed_ms={}",
        setup_start.elapsed().as_millis()
    ));

    if exit != 0 {
        logger.flush();
        DockerBuilder::stop_container(&container_name);
        DockerBuilder::remove_container(&container_name);
        let reason = classify_exit(exit, log_path);
        return Err(RunError::SessionSetupFailed {
            code: exit,
            reason,
            log_path: log_path.to_path_buf(),
        });
    }

    Ok(Session {
        container_name,
        image_id,
        scripts_dir,
        local,
        worktree_path,
        file_remote_path,
    })
}

fn container_args_from_run_options(
    name: String,
    repo_url: String,
    branch: String,
    runner_script: PathBuf,
    volume: String,
    agent_home: &'static str,
    local: bool,
    file_remote_path: Option<PathBuf>,
    opts: &RunOptions,
) -> ContainerArgs {
    ContainerArgs {
        name,
        repo_url,
        branch,
        runner_script,
        volume,
        agent_home,
        local,
        local_workspace_path: opts.local_workspace_path.clone(),
        worktree_path: opts.worktree_path.clone(),
        file_remote_path,
        timeout_secs: opts.timeout.map(|m| m as u64 * 60),
    }
}

#[allow(clippy::too_many_arguments)]
fn session_args(
    name: String,
    repo_url: String,
    scripts_dir: PathBuf,
    volume: String,
    agent_home: &'static str,
    local: bool,
    local_workspace_path: Option<PathBuf>,
    worktree_path: Option<PathBuf>,
    file_remote_path: Option<PathBuf>,
) -> crate::docker::SessionArgs {
    crate::docker::SessionArgs {
        name,
        repo_url,
        scripts_dir,
        volume,
        agent_home,
        local,
        local_workspace_path,
        worktree_path,
        file_remote_path,
    }
}

/// Execute one task inside an existing session container via `docker exec`.
/// Writes `task.sh` into the session's scripts dir (overwriting any prior task),
/// then streams output into a new per-task log/jsonl pair.
pub fn execute_in_session(
    config: &Config,
    repo_root: &Path,
    opts: RunOptions,
    session: &Session,
    tui: Option<&TuiHandle>,
) -> Result<Logger, RunError> {
    emit_event(
        &opts.event_sink,
        MrmouthEvent::StageChanged {
            stage: "Agent".to_string(),
        },
    );
    emit_event(
        &opts.event_sink,
        MrmouthEvent::RunLifecycle {
            action: RunAction::Starting,
            run_id: None,
            branch: None,
        },
    );

    // 0. Per-task log
    let log_dir = repo_root.join(&config.log_dir);
    fs::create_dir_all(&log_dir).map_err(|e| RunError::Io("creating log directory".into(), e))?;
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let log_filename = format!("run-{timestamp}.log");
    let log_path = log_dir.join(&log_filename);
    let logger = match tui {
        Some(t) => Logger::with_display_sink(&log_path, t.sender("AGENT SESSION"))
            .map_err(|e| RunError::Io("creating log file".into(), e))?,
        None => Logger::new(&log_path).map_err(|e| RunError::Io("creating log file".into(), e))?,
    };

    let branch = opts
        .branch
        .clone()
        .or_else(|| config.branch.clone())
        .unwrap_or_else(|| git_current_branch(repo_root).unwrap_or_else(|_| "main".into()));

    logger.log(&format!(
        "AGENT RUN  branch={branch}  {timestamp}  session={}",
        session.container_name
    ));
    emit_event(
        &opts.event_sink,
        MrmouthEvent::RunLabel {
            name: "branch".to_string(),
            value: branch.clone(),
        },
    );
    if let Some(path) = session
        .worktree_path
        .as_ref()
        .or(opts.worktree_path.as_ref())
    {
        emit_event(
            &opts.event_sink,
            MrmouthEvent::RunLabel {
                name: "workspace".to_string(),
                value: path.display().to_string(),
            },
        );
    }

    // 1. Generate task.sh for this iteration
    let prompt_text = match opts.prompt_override.as_deref() {
        Some(p) => p.to_string(),
        None => prompt::load_prompt(repo_root, Some(&logger)),
    };
    let prompt_text = if opts.prompt_override.is_none() {
        opts.repo_layout
            .as_ref()
            .map(|layout| layout.prepend_prompt_block(prompt_text.clone(), false))
            .unwrap_or(prompt_text)
    } else {
        prompt_text
    };
    write_task_script(
        config.agent,
        session.scripts_dir.path(),
        &opts.model,
        &prompt_text,
    )?;

    // 2. Exec task.sh
    let env_vars = vec![("BRANCH".to_string(), branch.clone())];
    let timeout_secs = opts.timeout.map(|m| m as u64 * 60);

    let jsonl_filename = format!("run-{timestamp}.jsonl");
    let jsonl_path = log_dir.join(&jsonl_filename);
    let jsonl_file =
        File::create(&jsonl_path).map_err(|e| RunError::Io("creating jsonl file".into(), e))?;
    let mut jsonl_writer = BufWriter::new(jsonl_file);
    let is_tty = logger.display_supports_color() || std::io::stdout().is_terminal();

    let container_start = std::time::Instant::now();
    emit_event(
        &opts.event_sink,
        MrmouthEvent::ContainerLifecycle {
            action: ContainerAction::Starting,
            name: session.container_name.clone(),
            image_id: Some(session.image_id.clone()),
            exit_code: None,
        },
    );
    let mut handle = crate::docker::DockerBuilder::exec_script(
        &session.container_name,
        "task.sh",
        &env_vars,
        timeout_secs,
    )
    .map_err(RunError::Docker)?;
    emit_event(
        &opts.event_sink,
        MrmouthEvent::ContainerLifecycle {
            action: ContainerAction::Started,
            name: session.container_name.clone(),
            image_id: Some(session.image_id.clone()),
            exit_code: None,
        },
    );

    // Cancel watcher
    let watcher_done = Arc::new(AtomicBool::new(false));
    let _done_guard = DoneGuard(Arc::clone(&watcher_done));
    let _cancel_watcher = if let Some(t) = tui {
        let flag = t.cancelled_flag();
        let done = Arc::clone(&watcher_done);
        let name = session.container_name.clone();
        Some(std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) && !done.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            if flag.load(Ordering::Relaxed) {
                DockerBuilder::stop_container(&name);
            }
        }))
    } else {
        None
    };

    match stream_display_mode(&opts) {
        StreamDisplayMode::Raw => {
            let stdout = std::io::stdout();
            handle
                .stream_output(|line| {
                    if should_suppress_stream_line(config.agent, line) {
                        return;
                    }
                    let _ = writeln!(stdout.lock(), "{line}");
                    let _ = writeln!(jsonl_writer, "{line}");
                    logger.log_file_only(line);
                })
                .map_err(RunError::Docker)?;
        }
        StreamDisplayMode::LifecycleJson => {
            handle
                .stream_output(|line| {
                    if should_suppress_stream_line(config.agent, line) {
                        return;
                    }
                    let _ = writeln!(jsonl_writer, "{line}");
                    logger.log_file_only(line);
                })
                .map_err(RunError::Docker)?;
        }
        StreamDisplayMode::Formatted => {
            let mut formatter = StreamFormatter::new(is_tty);
            handle
                .stream_output(|line| {
                    if should_suppress_stream_line(config.agent, line) {
                        return;
                    }
                    let _ = writeln!(jsonl_writer, "{line}");
                    if let Some(formatted) = stream_fmt::format_line(&mut formatter, line) {
                        logger.display(&formatted);
                        logger.log_file_only(&formatted);
                    }
                })
                .map_err(RunError::Docker)?;
        }
    }
    let _ = jsonl_writer.flush();

    let exit_code = handle.wait().map_err(RunError::Docker)?;
    watcher_done.store(true, Ordering::Relaxed);
    logger.log(&format!(
        "::mrmouth::timing phase=container-wall elapsed_ms={}",
        container_start.elapsed().as_millis()
    ));
    logger.log(&format!(
        "Task in session {} finished (exit code {exit_code}).",
        session.container_name
    ));
    emit_event(
        &opts.event_sink,
        MrmouthEvent::ContainerLifecycle {
            action: ContainerAction::Exited,
            name: session.container_name.clone(),
            image_id: Some(session.image_id.clone()),
            exit_code: Some(exit_code),
        },
    );

    // Update symlinks
    let latest_jsonl = log_dir.join("latest.jsonl");
    let latest_log = log_dir.join("latest.log");
    #[cfg(unix)]
    {
        atomic_symlink(&jsonl_filename, &latest_jsonl);
        atomic_symlink(&log_filename, &latest_log);
    }

    // Post-task host-side sync before extracting files. This keeps
    // self-produced, already-pushed Dockerfile edits from dirtying the host
    // before pull.
    logger.log("Post-run sync...");
    emit_event(
        &opts.event_sink,
        MrmouthEvent::RunLifecycle {
            action: RunAction::PullingChanges,
            run_id: Some(log_filename.clone()),
            branch: Some(branch.clone()),
        },
    );
    if !session.local && session.file_remote_path.is_none() {
        pull_code_changes(repo_root, &logger, None);
    }

    // Extract updated Dockerfile (agent may have edited it). Enables the caller
    // to detect Dockerfile changes and rebuild the session if needed.
    if !session.local {
        emit_event(
            &opts.event_sink,
            MrmouthEvent::RunLifecycle {
                action: RunAction::ExtractingDockerfile,
                run_id: Some(log_filename.clone()),
                branch: Some(branch.clone()),
            },
        );
        extract_updated_dockerfile(
            repo_root,
            &config.dockerfile,
            &session.container_name,
            &logger,
            None,
        );
    }

    emit_event(
        &opts.event_sink,
        MrmouthEvent::Sync {
            action: SyncAction::Starting,
            tool: SyncTool::Litebrite,
            detail: Some("post-run".to_string()),
        },
    );
    litebrite::init_and_sync(repo_root, Some(&logger));
    emit_event(
        &opts.event_sink,
        MrmouthEvent::Sync {
            action: SyncAction::Finished,
            tool: SyncTool::Litebrite,
            detail: Some("post-run".to_string()),
        },
    );
    logger.log(&format!("Done. Log saved: {}", log_path.display()));

    if exit_code != 0 {
        logger.flush();
        let reason = classify_exit(exit_code, &log_path);
        emit_event(
            &opts.event_sink,
            MrmouthEvent::LifecycleSummary {
                summary: LifecycleSummary::failed(
                    "run",
                    format!("container exited with code {exit_code}: {reason}"),
                )
                .branch(branch.clone())
                .workspace(run_workspace_label(
                    session.local,
                    session.worktree_path.as_deref(),
                ))
                .log_path(log_path.display().to_string())
                .jsonl_path(jsonl_path.display().to_string())
                .exit_code(exit_code)
                .next_action("inspect_log"),
            },
        );
        return Err(RunError::ContainerFailed {
            code: exit_code,
            reason,
            log_path: log_path.clone(),
        });
    }

    emit_event(
        &opts.event_sink,
        MrmouthEvent::RunLifecycle {
            action: RunAction::Finished,
            run_id: Some(log_filename),
            branch: Some(branch),
        },
    );

    Ok(logger)
}

fn run_workspace_label(local: bool, worktree_path: Option<&Path>) -> String {
    match worktree_path {
        Some(path) => path.display().to_string(),
        None if local => "/home/runner/workspace".to_string(),
        None => "cloned repository".to_string(),
    }
}

fn prompt_text_for_run(repo_root: &Path, opts: &RunOptions, logger: Option<&Logger>) -> String {
    match opts.prompt_override.as_deref() {
        Some(prompt) => prompt.to_string(),
        None => default_prompt_override(repo_root, opts, logger)
            .unwrap_or_else(|| prompt::load_prompt(repo_root, logger)),
    }
}

fn default_prompt_override(
    repo_root: &Path,
    opts: &RunOptions,
    logger: Option<&Logger>,
) -> Option<String> {
    opts.repo_layout.as_ref().and_then(|layout| {
        layout.is_split().then(|| {
            layout.prepend_prompt_block(
                prompt::load_prompt(repo_root, logger),
                opts.current_container,
            )
        })
    })
}

/// Stop and remove the session container. Scripts dir is cleaned up when
/// `session` is dropped.
pub fn stop_session(session: Session, logger: Option<&Logger>) {
    if let Some(l) = logger {
        l.log(&format!("Stopping session {}...", session.container_name));
    }
    DockerBuilder::stop_container(&session.container_name);
    DockerBuilder::remove_container(&session.container_name);
    // scripts_dir TempDir is dropped at function end
}

/// True when MRMOUTH_SKIP_PREFLIGHT=1 is set — a user-facing escape hatch for
/// overriding all preflight checks (tooling coherence, origin reachability,
/// docker, credentials, uncommitted changes, SSH agent).
fn preflight_skipped() -> bool {
    std::env::var("MRMOUTH_SKIP_PREFLIGHT").ok().as_deref() == Some("1")
}

fn spawn_current_container_watcher(
    tui: Option<&TuiHandle>,
    timeout_secs: Option<u64>,
    pid: u32,
    done: Arc<AtomicBool>,
) -> Option<std::thread::JoinHandle<()>> {
    let cancelled = tui.map(|t| t.cancelled_flag());
    if cancelled.is_none() && timeout_secs.is_none() {
        return None;
    }

    Some(std::thread::spawn(move || {
        let started = std::time::Instant::now();
        loop {
            if done.load(Ordering::Relaxed) {
                break;
            }
            let cancelled = cancelled
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Relaxed));
            let timed_out =
                timeout_secs.is_some_and(|secs| started.elapsed() >= Duration::from_secs(secs));
            if cancelled || timed_out {
                terminate_process(pid);
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }))
}

#[cfg(unix)]
fn terminate_process(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

#[cfg(not(unix))]
fn terminate_process(_pid: u32) {}

fn preflight(
    repo_root: &Path,
    agent: AgentKind,
    local: bool,
    current_container: bool,
    dockerfile_content: &str,
    logger: Option<&Logger>,
) -> Result<(), RunError> {
    if preflight_skipped() {
        return Ok(());
    }

    check_agent_credentials(agent)?;

    if current_container {
        check_current_container_tools(repo_root, agent)?;
    } else {
        let docker_check = Command::new("docker")
            .arg("info")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match docker_check {
            Ok(s) if s.success() => {}
            _ => {
                return Err(RunError::Preflight(
                    "Docker is not available. Is Docker running?".into(),
                ))
            }
        }

        // Best-effort diagnostic — never blocks preflight.
        check_disk_space(logger);

        check_tooling_coherence(repo_root, dockerfile_content)?;
    }

    if !local {
        let diff_status = Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "diff", "--quiet"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| RunError::Io("checking git diff".into(), e))?;
        let cached_status = Command::new("git")
            .args([
                "-C",
                &repo_root.to_string_lossy(),
                "diff",
                "--cached",
                "--quiet",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| RunError::Io("checking git diff --cached".into(), e))?;

        if !diff_status.success() || !cached_status.success() {
            return Err(RunError::Preflight(
                "Working tree has uncommitted changes. Commit or stash first.".into(),
            ));
        }

        if let Some(url) = git_remote_url(repo_root) {
            if is_ssh_remote(&url) {
                check_ssh_agent()?;
            }
            check_origin_reachable(repo_root)?;
        }
    }

    Ok(())
}

fn check_agent_credentials(agent: AgentKind) -> Result<(), RunError> {
    match agent {
        AgentKind::Claude => {
            let has_api_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
            let has_oauth = std::env::var("CLAUDE_CODE_OAUTH_TOKEN").is_ok();
            if !has_api_key && !has_oauth {
                return Err(RunError::Preflight(
                    "No Claude credentials found. Set ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN."
                        .into(),
                ));
            }
        }
        AgentKind::Codex => {
            // Codex may authenticate through OPENAI_API_KEY/CODEX_API_KEY or
            // through device auth stored in the persisted Docker home volume.
            // The host preflight cannot inspect that volume without risking a
            // false negative, so Codex auth is left to the Codex CLI.
        }
    }

    Ok(())
}

fn check_current_container_tools(repo_root: &Path, agent: AgentKind) -> Result<(), RunError> {
    for binary in ["git", agent.binary()] {
        if !command_exists(binary) {
            return Err(RunError::Preflight(format!(
                "current-container mode requires `{binary}` on PATH"
            )));
        }
    }

    for &(branch, binary) in TOOLING_PAIRS {
        if tooling_branch_exists(repo_root, branch) && !command_exists(binary) {
            return Err(RunError::Preflight(format!(
                "current-container mode requires `{binary}` on PATH because a {branch} branch exists"
            )));
        }
    }

    Ok(())
}

fn command_exists(binary: &str) -> bool {
    if binary.contains(std::path::MAIN_SEPARATOR) {
        return is_executable(Path::new(binary));
    }

    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| is_executable(&dir.join(binary)))
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Pairs of (branch name, binary name) for task-tracking tools that the runner
/// script invokes conditionally based on branch existence. If a tooling branch
/// exists but the Dockerfile doesn't build the binary, the runner would hit
/// exit 127 deep inside the container — catch it on the host instead.
const TOOLING_PAIRS: &[(&str, &str)] = &[("litebrite", "lb"), ("trapperkeeper", "trk")];

/// For each tooling branch present (locally or on origin), verify the effective
/// Dockerfile references the tool by name (e.g. contains "trapperkeeper"). If
/// it doesn't, the runner script's `trk init` / `lb init` call will hit exit
/// 127 deep inside the container — surface it here instead.
fn check_tooling_coherence(repo_root: &Path, dockerfile_content: &str) -> Result<(), RunError> {
    for &(branch, binary) in TOOLING_PAIRS {
        if !tooling_branch_exists(repo_root, branch) {
            continue;
        }
        if !dockerfile_content.contains(branch) {
            return Err(RunError::Preflight(format!(
                "Your Dockerfile does not build {binary}, but a {branch} branch exists. \
                 Either add the build stage (see DEFAULT_DOCKERFILE) or delete the branch. \
                 Set MRMOUTH_SKIP_PREFLIGHT=1 to override."
            )));
        }
    }
    Ok(())
}

/// Best-effort disk-space warning for `/var/lib/docker`. Emits a non-blocking
/// warning via the logger if free space is below the threshold. Any failure
/// (no df, non-Linux filesystem layout, parse mismatch) is silently ignored —
/// this is pure diagnostic help, not a hard gate.
fn check_disk_space(logger: Option<&Logger>) {
    const DOCKER_DIR: &str = "/var/lib/docker";
    const WARN_THRESHOLD_GB: u64 = 2;

    let Ok(output) = Command::new("df").args(["-P", DOCKER_DIR]).output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(avail_kb) = parse_df_available_kb(&text) else {
        return;
    };

    let avail_gb = avail_kb / (1024 * 1024);
    if avail_gb < WARN_THRESHOLD_GB {
        let msg = format!(
            "Warning: only {avail_gb} GB free at {DOCKER_DIR}. Docker build may fail with 'no space left on device'. Try 'docker system prune'."
        );
        crate::logger::log(logger, &msg);
    }
}

/// Parse the 4th column ("Available" in 1K-blocks) from `df -P` output.
/// Returns None if the output doesn't look like two lines of POSIX df.
fn parse_df_available_kb(text: &str) -> Option<u64> {
    // First data line after the header. `df -P` guarantees no line-wrapping.
    let line = text.lines().nth(1)?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    parts[3].parse::<u64>().ok()
}

/// Returns true if `branch` exists locally or on origin.
fn tooling_branch_exists(repo_root: &Path, branch: &str) -> bool {
    for reference in [
        format!("refs/heads/{branch}"),
        format!("refs/remotes/origin/{branch}"),
    ] {
        let ok = Command::new("git")
            .args([
                "-C",
                &repo_root.to_string_lossy(),
                "show-ref",
                "--quiet",
                &reference,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return true;
        }
    }
    false
}

/// Wall-clock timeout outcome for a subprocess invocation.
enum TimeoutOutcome {
    Completed(std::process::Output),
    TimedOut,
}

/// Run `cmd` with a wall-clock timeout, capturing stdout/stderr. On timeout,
/// the child is SIGKILL'd and reaped before returning `TimedOut`.
///
/// Note: this uses a `try_wait` polling loop, so the child's output must fit
/// inside the pipe buffer before it exits. Suitable for small-output commands
/// (git ls-remote HEAD, git rev-parse, etc.), not for streaming workloads.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> std::io::Result<TimeoutOutcome> {
    use std::io::Read;
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());
    let mut child = cmd.spawn()?;
    let start = std::time::Instant::now();

    loop {
        match child.try_wait()? {
            Some(status) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_end(&mut stdout);
                }
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_end(&mut stderr);
                }
                return Ok(TimeoutOutcome::Completed(std::process::Output {
                    status,
                    stdout,
                    stderr,
                }));
            }
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(TimeoutOutcome::TimedOut);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Probe origin with `git ls-remote --exit-code origin HEAD`. On unreachable
/// origin (DNS, VPN, SSH key, network), the container would otherwise fail
/// opaquely inside `git clone`; this catches it before docker build.
fn check_origin_reachable(repo_root: &Path) -> Result<(), RunError> {
    let mut cmd = Command::new("git");
    cmd.args([
        "-C",
        &repo_root.to_string_lossy(),
        "ls-remote",
        "--exit-code",
        "origin",
        "HEAD",
    ]);

    let outcome = run_with_timeout(cmd, Duration::from_secs(8))
        .map_err(|e| RunError::Io("running git ls-remote".into(), e))?;

    match outcome {
        TimeoutOutcome::Completed(out) if out.status.success() => Ok(()),
        TimeoutOutcome::Completed(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let msg = if stderr.is_empty() {
                format!(
                    "cannot reach origin (git ls-remote exit {})",
                    out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
                )
            } else {
                format!("cannot reach origin: {stderr}")
            };
            Err(RunError::Preflight(msg))
        }
        TimeoutOutcome::TimedOut => Err(RunError::Preflight(
            "cannot reach origin: git ls-remote did not respond within 8s (DNS, VPN, or SSH-agent issue?)".into(),
        )),
    }
}

/// True for SSH-form git remotes (git@host:... or ssh://...).
fn is_ssh_remote(url: &str) -> bool {
    url.starts_with("ssh://") || url.starts_with("git@")
}

/// Require SSH_AUTH_SOCK to be set and the socket path to exist on disk —
/// without it, the container has no way to authenticate git@ clones/pushes
/// and will either hang on a password prompt or fail with a permission error.
fn check_ssh_agent() -> Result<(), RunError> {
    let sock = std::env::var("SSH_AUTH_SOCK")
        .ok()
        .filter(|s| !s.is_empty());
    match sock {
        None => Err(RunError::Preflight(
            "origin is SSH (git@...) but SSH_AUTH_SOCK is not set — start an ssh-agent and ssh-add your key.".into(),
        )),
        Some(path) if !Path::new(&path).exists() => Err(RunError::Preflight(format!(
            "origin is SSH (git@...) but SSH_AUTH_SOCK={path} does not exist — start an ssh-agent and ssh-add your key."
        ))),
        Some(_) => Ok(()),
    }
}

fn git_remote_url(repo_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn configure_file_remote(repo_root: &Path) -> Result<(), RunError> {
    let status = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "config",
            "receive.denyCurrentBranch",
            "updateInstead",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| RunError::Io("configuring git receive policy".into(), e))?;
    if !status.success() {
        return Err(RunError::Preflight(
            "failed to set receive.denyCurrentBranch = updateInstead".into(),
        ));
    }
    Ok(())
}

/// Returns true if a litebrite or trapperkeeper branch exists locally but not on the remote.
fn has_local_only_tooling_branch(repo_root: &Path) -> bool {
    for branch in &["litebrite", "trapperkeeper"] {
        let local_exists = Command::new("git")
            .args([
                "-C",
                &repo_root.to_string_lossy(),
                "show-ref",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());

        if !local_exists {
            continue;
        }

        let remote_exists = Command::new("git")
            .args([
                "-C",
                &repo_root.to_string_lossy(),
                "show-ref",
                "--quiet",
                &format!("refs/remotes/origin/{branch}"),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());

        if !remote_exists {
            return true;
        }
    }
    false
}

fn git_current_branch(repo_root: &Path) -> Result<String, RunError> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "branch",
            "--show-current",
        ])
        .output()
        .map_err(|e| RunError::Io("getting current branch".into(), e))?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn write_runner_script(
    agent: AgentKind,
    repo_root: &Path,
    model: &str,
    prompt_override: Option<&str>,
    dockerfile_content: &str,
    dockerfile_path: &str,
    logger: Option<&Logger>,
) -> Result<tempfile::TempPath, RunError> {
    let prompt_text = match prompt_override {
        Some(p) => p.to_string(),
        None => prompt::load_prompt(repo_root, logger),
    };
    let escaped_prompt = prompt_text.replace('\'', "'\\''");
    let agent_name = agent.as_str();
    let agent_bin = agent.binary();
    let agent_version_line = agent.version_line();
    let agent_restore_block = agent.restore_block();
    let agent_command = agent.shell_command_with_disallowed_tools(model, &escaped_prompt);

    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
set -o errtrace
trap 'rc=$?; echo "::mrmouth::script-error rc=$rc line=$LINENO cmd=$BASH_COMMAND" >&2' ERR

_mm_t0=$(date +%s%N)
_mm_mark() {{ now=$(date +%s%N); echo "::mrmouth::timing phase=$1 elapsed_ms=$(( (now - _mm_t0) / 1000000 ))"; }}

# Run a tool init (lb/trk) that may exit non-zero with "already initialized"
# against an existing repo. Treat that one case as benign; real failures still
# propagate. Matches the host-side litebrite.rs silencing.
_mm_tool_init() {{
  local _out
  if _out=$("$@" 2>&1); then
    [ -n "$_out" ] && echo "$_out"
    return 0
  fi
  if echo "$_out" | grep -q "already initialized"; then
    return 0
  fi
  echo "$_out" >&2
  return 1
}}

_mm_mark script-start

# --- Tool versions (cheap, always-on diagnostic) ---
echo "::mrmouth::versions"
git --version || true
lb --version 2>/dev/null || echo "lb: not installed"
command -v trk >/dev/null && echo "trk: installed" || echo "trk: not installed"
{agent_version_line}
echo "::mrmouth::versions-end"
_mm_mark versions-done

repo_url="${{REPO_URL:-}}"
branch="${{BRANCH:-main}}"
work_dir="$HOME/workspace"
work_repo_dir="${{MRMOUTH_WORK_REPO:-$work_dir}}"

# --- Clone repo (skip if workspace already mounted) ---
if [ ! -d "$work_dir/.git" ]; then
  if [ -n "$repo_url" ]; then
    # Mark the host-repo volume as safe so git clone can read it
    git config --global --add safe.directory /host-repo
    echo "Cloning $repo_url (branch: $branch)..."
    git clone --branch "$branch" "$repo_url" "$work_dir"
  else
    echo "No repo URL and no .git — starting fresh in $work_dir"
    mkdir -p "$work_dir"
  fi
fi
cd "$work_dir"
git config --global --add safe.directory "$work_dir"
if [ "$work_repo_dir" != "$work_dir" ] && [ -e "$work_repo_dir" ]; then
  git config --global --add safe.directory "$work_repo_dir"
fi
_mm_mark clone-done

# --- Seed Dockerfile if absent (gives agent a file to read and modify) ---
dockerfile_path="$work_dir/__DOCKERFILE_REL_PATH__"
if [ ! -f "$dockerfile_path" ]; then
  mkdir -p "$(dirname "$dockerfile_path")"
  cat > "$dockerfile_path" << 'MRMOUTH_DOCKERFILE_EOF'
__DOCKERFILE_CONTENT__
MRMOUTH_DOCKERFILE_EOF
  echo "Seeded Dockerfile into workspace."
fi

# --- Initialize task tooling (requires git repo with matching branches) ---
if [ -d "$work_dir/.git" ]; then
  if git show-ref --quiet refs/heads/litebrite refs/remotes/origin/litebrite 2>/dev/null; then
    command -v lb >/dev/null || {{ echo "::mrmouth::missing-tool tool=lb reason=litebrite branch exists but binary not in image" >&2; exit 64; }}
    echo "Initializing litebrite..."
    _mm_tool_init lb init
    lb setup {agent_name} 2>/dev/null || true
    lb sync 2>/dev/null || true
  fi
  if git show-ref --quiet refs/heads/trapperkeeper refs/remotes/origin/trapperkeeper 2>/dev/null; then
    command -v trk >/dev/null || {{ echo "::mrmouth::missing-tool tool=trk reason=trapperkeeper branch exists but binary not in image" >&2; exit 64; }}
    echo "Initializing trapperkeeper..."
    _mm_tool_init trk init
    trk setup {agent_name} 2>/dev/null || true
    trk sync 2>/dev/null || true
  fi
fi
_mm_mark tooling-done

{agent_restore_block}

# --- Run agent ---
command -v {agent_bin} >/dev/null || {{ echo "::mrmouth::missing-tool tool={agent_bin} reason={agent_bin} binary not in image" >&2; exit 64; }}
echo "Starting agent run..."
_mm_mark agent-start
{agent_command}
_mm_mark agent-done

echo "Agent run complete."

# --- Belt-and-suspenders: force sync/push even if agent forgot ---
if [ -d "$work_dir/.git" ]; then
  echo "Post-agent cleanup: forcing sync and push..."
  lb sync 2>/dev/null || true
  trk sync 2>/dev/null || true
  git push 2>/dev/null || true
fi
_mm_mark script-end
"#
    );

    let script = script
        .replace("__DOCKERFILE_CONTENT__", dockerfile_content)
        .replace("__DOCKERFILE_REL_PATH__", dockerfile_path);

    let tmp = tempfile::NamedTempFile::new()
        .map_err(|e| RunError::Io("creating runner script".into(), e))?;

    // Close the write fd before mounting into Docker. Linux returns ETXTBSY when
    // execve() targets an inode that any process holds open for writing.
    // `into_temp_path()` closes the fd but keeps the deletion-on-drop guard.
    let tmp_path = tmp.into_temp_path();
    std::fs::write(&tmp_path, script.as_bytes())
        .map_err(|e| RunError::Io("writing runner script".into(), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&tmp_path, perms)
            .map_err(|e| RunError::Io("setting runner script permissions".into(), e))?;
    }

    Ok(tmp_path)
}

/// Write `setup.sh` into `scripts_dir`. This script runs once per session —
/// right after the container starts — and handles everything that doesn't
/// change between tasks: tool checks, initial clone, Dockerfile seeding,
/// litebrite/trapperkeeper init, claude.json restore.
#[allow(clippy::useless_format)]
pub(crate) fn write_setup_script(
    agent: AgentKind,
    scripts_dir: &Path,
    dockerfile_content: &str,
    dockerfile_path: &str,
) -> Result<(), RunError> {
    let agent_name = agent.as_str();
    let agent_bin = agent.binary();
    let agent_version_line = agent.version_line();
    let agent_restore_block = agent.restore_block();
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
set -o errtrace
trap 'rc=$?; echo "::mrmouth::script-error rc=$rc line=$LINENO cmd=$BASH_COMMAND" >&2' ERR

_mm_t0=$(date +%s%N)
_mm_mark() {{ now=$(date +%s%N); echo "::mrmouth::timing phase=$1 elapsed_ms=$(( (now - _mm_t0) / 1000000 ))"; }}

# Run a tool init (lb/trk) that may exit non-zero with "already initialized"
# against an existing repo. Treat that one case as benign; real failures still
# propagate. Matches the host-side litebrite.rs silencing.
_mm_tool_init() {{
  local _out
  if _out=$("$@" 2>&1); then
    [ -n "$_out" ] && echo "$_out"
    return 0
  fi
  if echo "$_out" | grep -q "already initialized"; then
    return 0
  fi
  echo "$_out" >&2
  return 1
}}

_mm_mark setup-start

# --- Tool versions ---
echo "::mrmouth::versions"
git --version || true
lb --version 2>/dev/null || echo "lb: not installed"
command -v trk >/dev/null && echo "trk: installed" || echo "trk: not installed"
{agent_version_line}
echo "::mrmouth::versions-end"
_mm_mark versions-done

repo_url="${{REPO_URL:-}}"
branch="${{BRANCH:-main}}"
work_dir="$HOME/workspace"
work_repo_dir="${{MRMOUTH_WORK_REPO:-$work_dir}}"

# --- Clone repo (skip if workspace already mounted) ---
if [ ! -d "$work_dir/.git" ]; then
  if [ -n "$repo_url" ]; then
    git config --global --add safe.directory /host-repo
    echo "Cloning $repo_url (branch: $branch)..."
    git clone --branch "$branch" "$repo_url" "$work_dir"
  else
    echo "No repo URL and no .git — starting fresh in $work_dir"
    mkdir -p "$work_dir"
  fi
fi
cd "$work_dir"
git config --global --add safe.directory "$work_dir"
if [ "$work_repo_dir" != "$work_dir" ] && [ -e "$work_repo_dir" ]; then
  git config --global --add safe.directory "$work_repo_dir"
fi
_mm_mark clone-done

# --- Seed Dockerfile if absent ---
dockerfile_path="$work_dir/__DOCKERFILE_REL_PATH__"
if [ ! -f "$dockerfile_path" ]; then
  mkdir -p "$(dirname "$dockerfile_path")"
  cat > "$dockerfile_path" << 'MRMOUTH_DOCKERFILE_EOF'
__DOCKERFILE_CONTENT__
MRMOUTH_DOCKERFILE_EOF
  echo "Seeded Dockerfile into workspace."
fi

# --- Initialize task tooling ---
if [ -d "$work_dir/.git" ]; then
  if git show-ref --quiet refs/heads/litebrite refs/remotes/origin/litebrite 2>/dev/null; then
    command -v lb >/dev/null || {{ echo "::mrmouth::missing-tool tool=lb reason=litebrite branch exists but binary not in image" >&2; exit 64; }}
    echo "Initializing litebrite..."
    _mm_tool_init lb init
    lb setup {agent_name} 2>/dev/null || true
    lb sync 2>/dev/null || true
  fi
  if git show-ref --quiet refs/heads/trapperkeeper refs/remotes/origin/trapperkeeper 2>/dev/null; then
    command -v trk >/dev/null || {{ echo "::mrmouth::missing-tool tool=trk reason=trapperkeeper branch exists but binary not in image" >&2; exit 64; }}
    echo "Initializing trapperkeeper..."
    _mm_tool_init trk init
    trk setup {agent_name} 2>/dev/null || true
    trk sync 2>/dev/null || true
  fi
fi
_mm_mark tooling-done

{agent_restore_block}

command -v {agent_bin} >/dev/null || {{ echo "::mrmouth::missing-tool tool={agent_bin} reason={agent_bin} binary not in image" >&2; exit 64; }}

_mm_mark setup-end
echo "::mrmouth::session-setup-complete"
"#
    );

    let script = script
        .replace("__DOCKERFILE_CONTENT__", dockerfile_content)
        .replace("__DOCKERFILE_REL_PATH__", dockerfile_path);

    write_script_file(scripts_dir, "setup.sh", &script)
}

/// Write `task.sh` into `scripts_dir`, overwriting any previous version.
/// Called once per epic iteration with the task-specific prompt. The script
/// switches to the task's branch (stashing any leftover state for safety),
/// runs the agent, and does post-run sync/push.
pub(crate) fn write_task_script(
    agent: AgentKind,
    scripts_dir: &Path,
    model: &str,
    prompt: &str,
) -> Result<(), RunError> {
    let escaped_prompt = prompt.replace('\'', "'\\''");
    let agent_command = agent.shell_command_with_disallowed_tools(model, &escaped_prompt);

    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
set -o errtrace
trap 'rc=$?; echo "::mrmouth::script-error rc=$rc line=$LINENO cmd=$BASH_COMMAND" >&2' ERR

_mm_t0=$(date +%s%N)
_mm_mark() {{ now=$(date +%s%N); echo "::mrmouth::timing phase=$1 elapsed_ms=$(( (now - _mm_t0) / 1000000 ))"; }}
_mm_mark task-start

work_dir="$HOME/workspace"
work_repo_dir="${{MRMOUTH_WORK_REPO:-$work_dir}}"
branch="${{BRANCH:-main}}"
cd "$work_dir"
if [ "$work_repo_dir" != "$work_dir" ] && [ -e "$work_repo_dir" ]; then
  git config --global --add safe.directory "$work_repo_dir"
fi

# --- Sync workspace with host ---
# Stash any leftover uncommitted state from a prior task (shouldn't happen if
# the agent committed cleanly, but don't let it block the next task).
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "::mrmouth::warning uncommitted changes from prior task — stashing"
  git stash push -u -m "mrmouth-leftover-$(date +%s)" || true
fi

echo "Fetching origin..."
git fetch origin --prune 2>&1 | grep -v '^From ' || true

# Switch to the task's branch (create local tracking branch if needed).
if git show-ref --quiet "refs/heads/$branch"; then
  git checkout "$branch"
elif git show-ref --quiet "refs/remotes/origin/$branch"; then
  git checkout -b "$branch" --track "origin/$branch"
else
  # Branch doesn't exist yet on origin — create a local one. Host will push later.
  git checkout -b "$branch"
fi
git pull --ff-only origin "$branch" 2>/dev/null || true
_mm_mark branch-ready

# Sync lb/trk state after branch switch (belt-and-suspenders).
lb sync 2>/dev/null || true
trk sync 2>/dev/null || true

# --- Run agent ---
echo "Starting agent run..."
_mm_mark agent-start
{agent_command}
_mm_mark agent-done

echo "Agent run complete."

# --- Post-run sync/push ---
if [ -d "$work_dir/.git" ]; then
  echo "Post-agent cleanup: forcing sync and push..."
  lb sync 2>/dev/null || true
  trk sync 2>/dev/null || true
  git push 2>/dev/null || true
fi
_mm_mark task-end
"#
    );

    write_script_file(scripts_dir, "task.sh", &script)
}

fn write_script_file(scripts_dir: &Path, filename: &str, content: &str) -> Result<(), RunError> {
    let path = scripts_dir.join(filename);
    std::fs::write(&path, content.as_bytes())
        .map_err(|e| RunError::Io(format!("writing {filename}"), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&path, perms)
            .map_err(|e| RunError::Io(format!("chmod {filename}"), e))?;
    }

    Ok(())
}

#[derive(Debug)]
pub enum RunError {
    Preflight(String),
    Docker(crate::docker::DockerError),
    Io(String, std::io::Error),
    ContainerFailed {
        code: i32,
        reason: String,
        log_path: PathBuf,
    },
    ProcessFailed {
        code: i32,
        reason: String,
        log_path: PathBuf,
    },
    SessionSetupFailed {
        code: i32,
        reason: String,
        log_path: PathBuf,
    },
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Preflight(msg) => write!(f, "preflight check failed: {msg}"),
            Self::Docker(e) => write!(f, "docker error: {e}"),
            Self::Io(ctx, e) => write!(f, "{ctx}: {e}"),
            Self::ContainerFailed { code, reason, .. } => {
                write!(f, "container exited with code {code}: {reason}")
            }
            Self::ProcessFailed { code, reason, .. } => {
                write!(f, "agent process exited with code {code}: {reason}")
            }
            Self::SessionSetupFailed { code, reason, .. } => {
                write!(f, "session setup failed (exit code {code}): {reason}")
            }
        }
    }
}

impl std::error::Error for RunError {}

impl RunError {
    /// Log file path, if this error produced one. Only ContainerFailed carries
    /// a log path today — other variants fail before or during container start.
    pub fn log_path(&self) -> Option<&Path> {
        match self {
            Self::ContainerFailed { log_path, .. } => Some(log_path),
            Self::ProcessFailed { log_path, .. } => Some(log_path),
            Self::SessionSetupFailed { log_path, .. } => Some(log_path),
            _ => None,
        }
    }

    /// Exit code from the container, if this error carries one.
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::ContainerFailed { code, .. } => Some(*code),
            Self::ProcessFailed { code, .. } => Some(*code),
            Self::SessionSetupFailed { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// Short one-line reason suitable for attempt-summary rendering.
    /// For container exits, prefixes with 'exit <code>'; otherwise reuses Display.
    pub fn short_reason(&self) -> String {
        match self {
            Self::ContainerFailed { code, reason, .. } => format!("exit {code} — {reason}"),
            Self::ProcessFailed { code, reason, .. } => format!("exit {code} — {reason}"),
            Self::SessionSetupFailed { code, reason, .. } => {
                format!("session setup exit {code} — {reason}")
            }
            other => other.to_string(),
        }
    }

    /// Build a debrief for printing after TUI teardown. For ContainerFailed,
    /// surfaces the reason, exit code, and log path (so the user sees the tail).
    /// Other variants just carry the Display message — they fail before a log
    /// file exists, so there's nothing to tail.
    pub fn debrief(&self) -> crate::debrief::FailureDebrief {
        match self {
            Self::ContainerFailed {
                code,
                reason,
                log_path,
            }
            | Self::ProcessFailed {
                code,
                reason,
                log_path,
            } => {
                let mut d = crate::debrief::FailureDebrief::new(format!(
                    "{} exited with code {code}: {reason}",
                    if matches!(self, Self::ContainerFailed { .. }) {
                        "container"
                    } else {
                        "agent process"
                    }
                ));
                d.exit_code = Some(*code);
                d.log_path = Some(log_path.clone());
                d
            }
            Self::SessionSetupFailed {
                code,
                reason,
                log_path,
            } => {
                let mut d = crate::debrief::FailureDebrief::new(format!(
                    "session setup failed (exit code {code}): {reason}"
                ));
                d.exit_code = Some(*code);
                d.log_path = Some(log_path.clone());
                d
            }
            other => crate::debrief::FailureDebrief::new(other.to_string()),
        }
    }
}

/// Classify a non-zero container exit code into a human-readable cause.
/// Scans the tail of the log for `::mrmouth::` markers (emitted by the runner
/// script) and well-known error fragments before falling back to exit-code
/// heuristics.
pub fn classify_exit(code: i32, log_path: &Path) -> String {
    let tail = read_log_tail(log_path, 8192);
    classify_exit_with_tail(code, &tail)
}

fn classify_exit_with_tail(code: i32, tail: &str) -> String {
    const MISSING_TOOL: &str = "::mrmouth::missing-tool ";
    const SCRIPT_ERROR: &str = "::mrmouth::script-error ";

    // Scan from the bottom up — the most recent marker wins.
    for line in tail.lines().rev() {
        if let Some(idx) = line.find(MISSING_TOOL) {
            let rest = &line[idx + MISSING_TOOL.len()..];
            let tool = parse_marker_field(rest, "tool=");
            let reason = parse_marker_tail(rest, "reason=");
            return match (tool, reason) {
                (Some(t), Some(r)) => format!("missing tool '{t}' — {r}"),
                (Some(t), None) => format!("missing tool '{t}'"),
                _ => "missing required tool in container".into(),
            };
        }
        if let Some(idx) = line.find(SCRIPT_ERROR) {
            let rest = &line[idx + SCRIPT_ERROR.len()..];
            let line_no = parse_marker_field(rest, "line=");
            let cmd = parse_marker_tail(rest, "cmd=");
            return match (line_no, cmd) {
                (Some(l), Some(c)) => format!("runner script error at line {l}: {c}"),
                (Some(l), None) => format!("runner script error at line {l}"),
                (None, Some(c)) => format!("runner script error: {c}"),
                _ => "runner script error".into(),
            };
        }
    }

    // Pattern-based heuristics — the runner script may not have caught it,
    // but the log still has tell-tale fragments.
    if let Some(line) = tail.lines().rev().find(|l| l.contains("command not found")) {
        let snippet = line.trim();
        return format!("missing command in container — {snippet}");
    }
    if tail.contains("Killed") || tail.contains("OOMKilled") {
        return "container killed (likely OOM)".into();
    }
    if tail.contains("context deadline exceeded") {
        return "timed out (context deadline exceeded)".into();
    }

    // Code-based fallback.
    match code {
        64 => "missing required tool in container (runner script guard fired — see log)".into(),
        124 => "timed out (timeout wrapper)".into(),
        126 => "permission denied / not executable".into(),
        127 => "missing command in container (check your Dockerfile includes all expected tools)"
            .into(),
        137 => "container killed (likely OOM)".into(),
        143 => "timed out (SIGTERM)".into(),
        _ => format!("unrecognised exit code {code}"),
    }
}

/// Read up to `n_bytes` from the end of `log_path` as a string. Returns "" on
/// any I/O error so callers can keep classifying with what they have.
fn read_log_tail(log_path: &Path, n_bytes: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(log_path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(n_bytes as u64);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::with_capacity(n_bytes);
    let _ = file.take(n_bytes as u64).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Extract a single-token value for `key=` from a marker payload.
/// Returns the substring up to the next whitespace.
fn parse_marker_field(payload: &str, key: &str) -> Option<String> {
    let pos = payload.find(key)?;
    let value_start = pos + key.len();
    let rest = &payload[value_start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Extract a multi-word value for `key=` that runs to the end of the payload.
/// Used for free-form fields like `reason=...` and `cmd=...`.
fn parse_marker_tail(payload: &str, key: &str) -> Option<String> {
    let pos = payload.find(key)?;
    let value_start = pos + key.len();
    Some(payload[value_start..].trim().to_string())
}

fn should_suppress_stream_line(agent: AgentKind, line: &str) -> bool {
    matches!(agent, AgentKind::Codex) && line.trim() == "Reading additional input from stdin..."
}

/// Atomically replace a symlink by creating a temp link and renaming over the target.
#[cfg(unix)]
fn atomic_symlink(target: &str, link_path: &std::path::PathBuf) {
    use std::os::unix::fs as unix_fs;
    let tmp = link_path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    if unix_fs::symlink(target, &tmp).is_ok() && fs::rename(&tmp, link_path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker;
    use std::io::Read as _;

    const TEST_DOCKERFILE: &str = docker::DEFAULT_DOCKERFILE;
    const TEST_DOCKERFILE_PATH: &str = ".mrmouth/Dockerfile";

    #[test]
    fn run_reporter_emits_status_messages_and_preserves_log_file() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("run.log");
        let logger = Logger::new(&log_path).unwrap();
        let sink = crate::events::RecordingEventSink::default();
        let sink_handle = EventSinkHandle::new(sink.clone());
        let reporter = RunReporter::new(Some(&sink_handle), None);

        reporter.log(&logger, "Checking preflight conditions...");
        logger.flush();

        assert_eq!(
            sink.events(),
            vec![MrmouthEvent::Message {
                level: MessageLevel::Info,
                text: "Checking preflight conditions...".to_string(),
                target: MessageTarget::Agent,
            }]
        );
        assert_eq!(
            std::fs::read_to_string(log_path).unwrap(),
            "Checking preflight conditions...\n"
        );
    }

    #[test]
    fn event_sink_without_json_events_keeps_formatted_stream_mode() {
        let sink = crate::events::RecordingEventSink::default();
        let opts = RunOptions {
            raw: false,
            json_events: false,
            model: "sonnet".to_string(),
            timeout: None,
            local: false,
            current_container: false,
            local_workspace_path: None,
            worktree_path: None,
            repo_layout: None,
            prompt_override: None,
            branch: None,
            event_sink: Some(EventSinkHandle::new(sink)),
        };

        assert_eq!(stream_display_mode(&opts), StreamDisplayMode::Formatted);
    }

    #[test]
    fn json_events_mode_suppresses_formatted_stream_display() {
        let opts = RunOptions {
            raw: false,
            json_events: true,
            model: "sonnet".to_string(),
            timeout: None,
            local: false,
            current_container: false,
            local_workspace_path: None,
            worktree_path: None,
            repo_layout: None,
            prompt_override: None,
            branch: None,
            event_sink: None,
        };

        assert_eq!(stream_display_mode(&opts), StreamDisplayMode::LifecycleJson);
    }

    #[test]
    fn run_options_worktree_flows_into_container_args() {
        let worktree = PathBuf::from("/host/service");
        let opts = RunOptions {
            raw: false,
            json_events: false,
            model: "sonnet".to_string(),
            timeout: Some(7),
            local: false,
            current_container: false,
            local_workspace_path: None,
            worktree_path: Some(worktree.clone()),
            repo_layout: None,
            prompt_override: None,
            branch: None,
            event_sink: None,
        };

        let args = container_args_from_run_options(
            "run-test".to_string(),
            "git@example.com:org/repo.git".to_string(),
            "feature".to_string(),
            PathBuf::from("/tmp/run.sh"),
            "mrmouth-home".to_string(),
            "/home/runner/.codex",
            false,
            None,
            &opts,
        );

        assert_eq!(args.worktree_path.as_deref(), Some(worktree.as_path()));
        assert_eq!(args.local_workspace_path, None);
        assert!(!args.local);
        assert_eq!(args.timeout_secs, Some(420));
    }

    #[test]
    fn session_args_preserve_worktree_mapping() {
        let worktree = PathBuf::from("/host/service");
        let args = session_args(
            "session-test".to_string(),
            "git@example.com:org/repo.git".to_string(),
            PathBuf::from("/tmp/scripts"),
            "mrmouth-home".to_string(),
            "/home/runner/.codex",
            false,
            None,
            Some(worktree.clone()),
            None,
        );

        assert_eq!(args.worktree_path.as_deref(), Some(worktree.as_path()));
        assert_eq!(args.local_workspace_path, None);
        assert!(!args.local);
    }

    #[test]
    fn runner_script_contains_model() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "sonnet",
            None,
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert!(content.contains("--model sonnet"));
    }

    #[test]
    fn runner_script_contains_lb_sync() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            None,
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        let init_pos = content.find("lb init").unwrap();
        let sync_pos = content.find("lb sync 2>/dev/null || true").unwrap();
        assert!(sync_pos > init_pos, "lb sync should come after lb init");
    }

    #[test]
    fn runner_script_uses_prompt_override() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            Some("custom prompt here"),
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert!(content.contains("custom prompt here"));
    }

    #[test]
    fn runner_script_escapes_single_quotes_in_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            Some("don't break"),
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert!(content.contains(r"don'\''t break"));
    }

    #[test]
    fn runner_script_is_executable() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            None,
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&tmp).unwrap().permissions();
            assert_eq!(perms.mode() & 0o755, 0o755);
        }
    }

    #[test]
    fn runner_script_has_shebang() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            None,
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert!(content.starts_with("#!/usr/bin/env bash"));
    }

    #[test]
    fn runner_script_seeds_dockerfile() {
        let dir = tempfile::tempdir().unwrap();
        let dockerfile = "FROM node:22\nRUN echo hello\nARG HOST_UID=${HOST_UID}";
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            None,
            dockerfile,
            ".mrmouth/Dockerfile",
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert!(content.contains("MRMOUTH_DOCKERFILE_EOF"));
        assert!(content.contains("FROM node:22"));
        assert!(
            content.contains("ARG HOST_UID=${HOST_UID}"),
            "Docker ARG syntax must be preserved literally"
        );
        assert!(content.contains(".mrmouth/Dockerfile"));
    }

    #[test]
    fn runner_script_has_err_trap() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            None,
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert!(content.contains("set -o errtrace"));
        assert!(content.contains("::mrmouth::script-error"));
        assert!(content.contains("$LINENO"));
        assert!(content.contains("$BASH_COMMAND"));
        assert!(content.contains("' ERR"));
    }

    #[test]
    fn runner_script_echoes_tool_versions() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            None,
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();

        let begin = content
            .find("::mrmouth::versions")
            .expect("missing versions begin marker");
        let end = content
            .find("::mrmouth::versions-end")
            .expect("missing versions end marker");
        assert!(begin < end, "versions begin must precede end");

        let block = &content[begin..end];
        assert!(block.contains("git --version"));
        assert!(block.contains("lb --version"));
        assert!(block.contains("command -v trk"));
        assert!(block.contains("trk: installed"));
        assert!(block.contains("claude --version"));
        assert!(block.contains("lb: not installed"));
        assert!(block.contains("trk: not installed"));
        assert!(block.contains("claude: not installed"));

        // Versions must print before any tool is actually invoked for setup.
        let lb_init = content.find("lb init").expect("missing lb init");
        assert!(end < lb_init, "version echo must come before tool setup");
    }

    #[test]
    fn runner_script_guards_lb_binary() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            None,
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        let guard_pos = content
            .find("command -v lb >/dev/null")
            .expect("missing lb guard");
        let init_pos = content.find("lb init").expect("missing lb init");
        assert!(guard_pos < init_pos, "lb guard must precede lb init");
        assert!(content.contains("::mrmouth::missing-tool tool=lb"));
        assert!(content.contains("exit 64"));
    }

    #[test]
    fn runner_script_guards_trk_binary() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            None,
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        let guard_pos = content
            .find("command -v trk >/dev/null")
            .expect("missing trk guard");
        let init_pos = content.find("trk init").expect("missing trk init");
        assert!(guard_pos < init_pos, "trk guard must precede trk init");
        assert!(content.contains("::mrmouth::missing-tool tool=trk"));
    }

    #[test]
    fn runner_script_guards_claude_binary() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            None,
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        let guard_pos = content
            .find("command -v claude >/dev/null")
            .expect("missing claude guard");
        let claude_pos = content
            .find("claude -p --dangerously-skip-permissions")
            .expect("missing claude invocation");
        assert!(
            guard_pos < claude_pos,
            "claude guard must precede claude invocation"
        );
        assert!(content.contains("::mrmouth::missing-tool tool=claude"));
    }

    #[test]
    fn runner_script_supports_codex_agent() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Codex,
            dir.path(),
            "gpt-5.2",
            Some("do it"),
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert!(content.contains("codex --version"));
        assert!(content.contains("lb setup codex"));
        assert!(content.contains("trk setup codex"));
        assert!(content.contains("command -v codex >/dev/null"));
        assert!(content.contains("codex exec --json --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check --model gpt-5.2 'do it' </dev/null"));
        assert!(!content.contains("claude -p --dangerously-skip-permissions"));
    }

    #[test]
    fn runner_script_omits_empty_codex_model() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = write_runner_script(
            crate::agent::AgentKind::Codex,
            dir.path(),
            "",
            Some("do it"),
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
            None,
        )
        .unwrap();
        let mut content = String::new();
        File::open(&tmp)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert!(content.contains("codex exec --json --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check 'do it' </dev/null"));
        assert!(!content.contains("--model opus"));
    }

    #[test]
    fn classify_recognises_missing_tool_marker() {
        let tail = "Initializing trapperkeeper...\n\
                    ::mrmouth::missing-tool tool=trk reason=trapperkeeper branch exists but binary not in image\n";
        let msg = classify_exit_with_tail(64, tail);
        assert!(msg.contains("missing tool 'trk'"), "got: {msg}");
        assert!(
            msg.contains("trapperkeeper branch exists but binary not in image"),
            "got: {msg}"
        );
    }

    #[test]
    fn classify_recognises_script_error_marker() {
        let tail = "::mrmouth::script-error rc=1 line=42 cmd=git clone foo bar\n";
        let msg = classify_exit_with_tail(1, tail);
        assert!(msg.contains("line 42"), "got: {msg}");
        assert!(msg.contains("git clone foo bar"), "got: {msg}");
    }

    #[test]
    fn classify_marker_takes_precedence_over_code() {
        // Even with code 127, an explicit marker should win.
        let tail = "::mrmouth::missing-tool tool=lb reason=litebrite branch exists but binary not in image\n";
        let msg = classify_exit_with_tail(127, tail);
        assert!(msg.contains("missing tool 'lb'"), "got: {msg}");
    }

    #[test]
    fn classify_most_recent_marker_wins() {
        // If two markers appear, the one nearer the bottom (most recent) wins.
        let tail = "::mrmouth::missing-tool tool=lb reason=lb missing\n\
                    ::mrmouth::missing-tool tool=trk reason=trk missing\n";
        let msg = classify_exit_with_tail(64, tail);
        assert!(msg.contains("'trk'"), "got: {msg}");
        assert!(!msg.contains("'lb'"), "got: {msg}");
    }

    #[test]
    fn classify_falls_back_to_command_not_found_pattern() {
        let tail = "Initializing litebrite...\n\
                    /run.sh: line 30: lb: command not found\n";
        let msg = classify_exit_with_tail(127, tail);
        assert!(msg.contains("missing command in container"), "got: {msg}");
        assert!(msg.contains("command not found"), "got: {msg}");
    }

    #[test]
    fn classify_falls_back_to_exit_code() {
        assert!(classify_exit_with_tail(127, "").contains("missing command"));
        assert!(classify_exit_with_tail(137, "").contains("OOM"));
        assert!(classify_exit_with_tail(143, "").contains("SIGTERM"));
        assert!(classify_exit_with_tail(124, "").contains("timeout"));
        assert!(classify_exit_with_tail(126, "").contains("permission"));
        assert!(classify_exit_with_tail(64, "").contains("runner script guard"));
        assert!(classify_exit_with_tail(99, "").contains("99"));
    }

    #[test]
    fn classify_handles_unknown_code_with_no_log() {
        let msg = classify_exit_with_tail(42, "");
        assert!(msg.contains("42"));
    }

    #[test]
    fn classify_marker_without_fields_degrades_gracefully() {
        // Marker matched but with no recognised key=value fields.
        let tail = "::mrmouth::missing-tool unknown=junk\n";
        let msg = classify_exit_with_tail(64, tail);
        assert_eq!(msg, "missing required tool in container");
    }

    #[test]
    fn read_log_tail_returns_empty_for_missing_file() {
        let path = std::path::PathBuf::from("/nonexistent/path/to/log");
        assert_eq!(read_log_tail(&path, 1024), "");
    }

    #[test]
    fn read_log_tail_returns_last_n_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.log");
        std::fs::write(&path, "0123456789abcdef").unwrap();
        let tail = read_log_tail(&path, 5);
        assert_eq!(tail, "bcdef");
    }

    #[test]
    fn ssh_remote_detection() {
        assert!(is_ssh_remote("git@github.com:org/repo.git"));
        assert!(is_ssh_remote("ssh://git@github.com/org/repo.git"));
        assert!(!is_ssh_remote("https://github.com/org/repo.git"));
        assert!(!is_ssh_remote("file:///host-repo"));
        assert!(!is_ssh_remote(""));
    }

    #[test]
    fn check_ssh_agent_missing_env_errs() {
        // Serialize with other env-mutating tests to avoid flakiness under parallel cargo test.
        let _guard = env_lock();
        let prev = std::env::var_os("SSH_AUTH_SOCK");
        std::env::remove_var("SSH_AUTH_SOCK");
        let result = check_ssh_agent();
        if let Some(v) = prev {
            std::env::set_var("SSH_AUTH_SOCK", v);
        }
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("SSH_AUTH_SOCK is not set"));
    }

    #[test]
    fn check_ssh_agent_nonexistent_socket_errs() {
        let _guard = env_lock();
        let prev = std::env::var_os("SSH_AUTH_SOCK");
        std::env::set_var(
            "SSH_AUTH_SOCK",
            "/tmp/definitely-not-a-real-socket-xyz-mrmouth",
        );
        let result = check_ssh_agent();
        match prev {
            Some(v) => std::env::set_var("SSH_AUTH_SOCK", v),
            None => std::env::remove_var("SSH_AUTH_SOCK"),
        }
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("does not exist"), "got: {msg}");
    }

    #[test]
    fn check_ssh_agent_existing_socket_ok() {
        let _guard = env_lock();
        let dir = tempfile::tempdir().unwrap();
        let fake_sock = dir.path().join("agent.sock");
        std::fs::write(&fake_sock, "").unwrap();
        let prev = std::env::var_os("SSH_AUTH_SOCK");
        std::env::set_var("SSH_AUTH_SOCK", &fake_sock);
        let result = check_ssh_agent();
        match prev {
            Some(v) => std::env::set_var("SSH_AUTH_SOCK", v),
            None => std::env::remove_var("SSH_AUTH_SOCK"),
        }
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn run_with_timeout_completes_fast_command() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf out; printf err >&2; exit 0"]);
        let out = match run_with_timeout(cmd, Duration::from_secs(5)).unwrap() {
            TimeoutOutcome::Completed(o) => o,
            TimeoutOutcome::TimedOut => panic!("fast command unexpectedly timed out"),
        };
        assert!(out.status.success());
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
    }

    #[test]
    #[cfg(unix)]
    fn run_with_timeout_kills_slow_command() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let start = std::time::Instant::now();
        let outcome = run_with_timeout(cmd, Duration::from_millis(300)).unwrap();
        let elapsed = start.elapsed();
        assert!(matches!(outcome, TimeoutOutcome::TimedOut));
        // Should return well before the 30s sleep completes.
        assert!(
            elapsed < Duration::from_secs(5),
            "run_with_timeout blocked for {elapsed:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_with_timeout_reports_nonzero_exit() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo boom >&2; exit 42"]);
        let out = match run_with_timeout(cmd, Duration::from_secs(5)).unwrap() {
            TimeoutOutcome::Completed(o) => o,
            TimeoutOutcome::TimedOut => panic!("unexpected timeout"),
        };
        assert_eq!(out.status.code(), Some(42));
        assert!(String::from_utf8_lossy(&out.stderr).contains("boom"));
    }

    #[cfg(unix)]
    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-C", &dir.to_string_lossy()])
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git run");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    #[cfg(unix)]
    fn check_origin_reachable_succeeds_for_local_remote() {
        let origin_dir = tempfile::tempdir().unwrap();
        git(origin_dir.path(), &["init", "-q"]);
        std::fs::write(origin_dir.path().join("README"), "hi").unwrap();
        git(origin_dir.path(), &["add", "README"]);
        git(origin_dir.path(), &["commit", "-q", "-m", "init"]);

        let work_dir = tempfile::tempdir().unwrap();
        git(work_dir.path(), &["init", "-q"]);
        git(
            work_dir.path(),
            &[
                "remote",
                "add",
                "origin",
                &origin_dir.path().to_string_lossy(),
            ],
        );

        check_origin_reachable(work_dir.path()).expect("local origin should be reachable");
    }

    #[test]
    #[cfg(unix)]
    fn check_origin_reachable_fails_for_missing_remote() {
        let work_dir = tempfile::tempdir().unwrap();
        git(work_dir.path(), &["init", "-q"]);
        git(
            work_dir.path(),
            &["remote", "add", "origin", "/nonexistent/path/mrmouth-y5z9"],
        );

        let err = check_origin_reachable(work_dir.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("cannot reach origin"), "got: {msg}");
    }

    #[cfg(unix)]
    fn make_repo_with_branch(branch: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("f"), "x").unwrap();
        git(dir.path(), &["add", "f"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        if let Some(b) = branch {
            git(dir.path(), &["branch", b]);
        }
        dir
    }

    #[test]
    #[cfg(unix)]
    fn check_tooling_coherence_ok_when_no_branches() {
        let dir = make_repo_with_branch(None);
        check_tooling_coherence(dir.path(), "FROM node:22\n").expect("no tooling branches → pass");
    }

    #[test]
    #[cfg(unix)]
    fn check_tooling_coherence_ok_when_dockerfile_mentions_branch() {
        let dir = make_repo_with_branch(Some("trapperkeeper"));
        let dockerfile =
            "FROM rust\nRUN cargo install --git https://example.com/trapperkeeper.git\n";
        check_tooling_coherence(dir.path(), dockerfile)
            .expect("dockerfile mentions trapperkeeper → pass");
    }

    #[test]
    #[cfg(unix)]
    fn check_tooling_coherence_fails_when_dockerfile_missing_trk() {
        let dir = make_repo_with_branch(Some("trapperkeeper"));
        let err = check_tooling_coherence(dir.path(), "FROM node:22\n").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("does not build trk"), "got: {msg}");
        assert!(msg.contains("trapperkeeper branch exists"), "got: {msg}");
        assert!(msg.contains("MRMOUTH_SKIP_PREFLIGHT"), "got: {msg}");
    }

    #[test]
    #[cfg(unix)]
    fn check_tooling_coherence_fails_when_dockerfile_missing_lb() {
        let dir = make_repo_with_branch(Some("litebrite"));
        let err = check_tooling_coherence(dir.path(), "FROM node:22\n").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("does not build lb"), "got: {msg}");
        assert!(msg.contains("litebrite branch exists"), "got: {msg}");
    }

    #[test]
    #[cfg(unix)]
    fn check_tooling_coherence_passes_with_default_dockerfile() {
        // Sanity check: DEFAULT_DOCKERFILE contains both tooling words.
        let dir = make_repo_with_branch(Some("litebrite"));
        git(dir.path(), &["branch", "trapperkeeper"]);
        check_tooling_coherence(dir.path(), docker::DEFAULT_DOCKERFILE)
            .expect("default Dockerfile must satisfy coherence for both tools");
    }

    #[test]
    fn preflight_skipped_reads_env_as_one() {
        let _guard = env_lock();
        let prev = std::env::var_os("MRMOUTH_SKIP_PREFLIGHT");

        std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", "1");
        let skip_one = preflight_skipped();
        std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", "0");
        let skip_zero = preflight_skipped();
        std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", "true");
        let skip_true = preflight_skipped();
        std::env::remove_var("MRMOUTH_SKIP_PREFLIGHT");
        let skip_unset = preflight_skipped();

        match prev {
            Some(v) => std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", v),
            None => std::env::remove_var("MRMOUTH_SKIP_PREFLIGHT"),
        }

        assert!(skip_one, "=1 should skip");
        assert!(!skip_zero, "=0 should not skip");
        assert!(!skip_true, "=true should not skip (strict match on 1)");
        assert!(!skip_unset, "unset should not skip");
    }

    #[test]
    #[cfg(unix)]
    fn preflight_short_circuits_when_skip_env_set() {
        let _guard = env_lock();
        let dir = make_repo_with_branch(Some("trapperkeeper"));

        let prev_skip = std::env::var_os("MRMOUTH_SKIP_PREFLIGHT");
        std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", "1");
        // Dockerfile lacks trk, but env skip must bypass that (and all other checks).
        let result = preflight(
            dir.path(),
            AgentKind::Claude,
            true,
            false,
            "FROM scratch\n",
            None,
        );
        match prev_skip {
            Some(v) => std::env::set_var("MRMOUTH_SKIP_PREFLIGHT", v),
            None => std::env::remove_var("MRMOUTH_SKIP_PREFLIGHT"),
        }
        assert!(
            result.is_ok(),
            "skip env should bypass all preflight failures"
        );
    }

    #[test]
    fn claude_credentials_require_claude_env() {
        let _guard = env_lock();
        let prev_api_key = std::env::var_os("ANTHROPIC_API_KEY");
        let prev_oauth = std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");

        let result = check_agent_credentials(AgentKind::Claude);

        match prev_api_key {
            Some(v) => std::env::set_var("ANTHROPIC_API_KEY", v),
            None => std::env::remove_var("ANTHROPIC_API_KEY"),
        }
        match prev_oauth {
            Some(v) => std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", v),
            None => std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN"),
        }

        let err = result.unwrap_err();
        assert!(format!("{err}").contains("No Claude credentials found"));
    }

    #[test]
    fn codex_credentials_do_not_require_claude_env() {
        let _guard = env_lock();
        let prev_api_key = std::env::var_os("ANTHROPIC_API_KEY");
        let prev_oauth = std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");

        let result = check_agent_credentials(AgentKind::Codex);

        match prev_api_key {
            Some(v) => std::env::set_var("ANTHROPIC_API_KEY", v),
            None => std::env::remove_var("ANTHROPIC_API_KEY"),
        }
        match prev_oauth {
            Some(v) => std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", v),
            None => std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN"),
        }

        assert!(result.is_ok());
    }

    #[test]
    fn parse_df_available_kb_linux_format() {
        let text = "Filesystem     1024-blocks       Used   Available Capacity Mounted on\n\
                    /dev/nvme0n1p2    499963392  337834220   137129788      72% /\n";
        assert_eq!(parse_df_available_kb(text), Some(137129788));
    }

    #[test]
    fn parse_df_available_kb_macos_format() {
        // macOS df -P has 512-byte blocks by default without -k, but -P uses 1024-blocks too.
        let text =
            "Filesystem  1024-blocks     Used Available Capacity iused ifree %iused Mounted on\n\
                    /dev/disk1s1  487893504  63218928 420000000    14% 500000 1000000   33%   /\n";
        assert_eq!(parse_df_available_kb(text), Some(420000000));
    }

    #[test]
    fn parse_df_available_kb_empty_input() {
        assert_eq!(parse_df_available_kb(""), None);
        assert_eq!(parse_df_available_kb("only a header line\n"), None);
    }

    #[test]
    fn parse_df_available_kb_garbled_input() {
        // Non-numeric 4th column.
        let text = "h1 h2 h3 h4 h5 h6\n\
                    a  b  c  not-a-number e f\n";
        assert_eq!(parse_df_available_kb(text), None);
    }

    #[test]
    fn suppresses_codex_stdin_status_line_only_for_codex() {
        assert!(should_suppress_stream_line(
            AgentKind::Codex,
            "Reading additional input from stdin..."
        ));
        assert!(should_suppress_stream_line(
            AgentKind::Codex,
            "  Reading additional input from stdin...  "
        ));
        assert!(!should_suppress_stream_line(
            AgentKind::Claude,
            "Reading additional input from stdin..."
        ));
        assert!(!should_suppress_stream_line(
            AgentKind::Codex,
            "Reading project files..."
        ));
    }

    #[test]
    fn parse_df_available_kb_short_line() {
        let text = "h1 h2 h3\n\
                    a b c\n";
        assert_eq!(parse_df_available_kb(text), None);
    }

    #[test]
    fn check_disk_space_silently_ignores_missing_dir() {
        // /var/lib/docker typically doesn't exist on the test host — df returns non-zero.
        // The function must not panic and must not log anything mandatory.
        check_disk_space(None);
    }

    #[test]
    fn run_error_debrief_container_failure_carries_code_and_log() {
        let err = RunError::ContainerFailed {
            code: 127,
            reason: "missing command in container — trk: command not found".into(),
            log_path: std::path::PathBuf::from("/logs/run-abc.log"),
        };
        let d = err.debrief();
        assert_eq!(d.exit_code, Some(127));
        assert_eq!(d.log_path.as_deref(), Some(Path::new("/logs/run-abc.log")));
        assert!(
            d.message.contains("container exited with code 127"),
            "got: {}",
            d.message
        );
        assert!(
            d.message.contains("trk: command not found"),
            "got: {}",
            d.message
        );
    }

    #[test]
    fn run_error_debrief_preflight_has_no_log_or_exit() {
        let err = RunError::Preflight("Docker is not available".into());
        let d = err.debrief();
        assert!(d.message.contains("Docker is not available"));
        assert!(d.exit_code.is_none());
        assert!(d.log_path.is_none());
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn setup_script_contains_dockerfile_content() {
        let dir = tempfile::tempdir().unwrap();
        write_setup_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
        )
        .unwrap();
        let content = std::fs::read_to_string(dir.path().join("setup.sh")).unwrap();
        assert!(
            content.contains("FROM rust:slim"),
            "setup.sh should embed Dockerfile content"
        );
        assert!(content.contains(TEST_DOCKERFILE_PATH));
    }

    #[test]
    fn setup_script_has_shebang_and_timing_markers() {
        let dir = tempfile::tempdir().unwrap();
        write_setup_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
        )
        .unwrap();
        let content = std::fs::read_to_string(dir.path().join("setup.sh")).unwrap();
        assert!(content.starts_with("#!/usr/bin/env bash"));
        assert!(content.contains("_mm_mark setup-start"));
        assert!(content.contains("_mm_mark setup-end"));
        assert!(content.contains("::mrmouth::session-setup-complete"));
    }

    #[test]
    fn setup_script_supports_codex_agent() {
        let dir = tempfile::tempdir().unwrap();
        write_setup_script(
            crate::agent::AgentKind::Codex,
            dir.path(),
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
        )
        .unwrap();
        let content = std::fs::read_to_string(dir.path().join("setup.sh")).unwrap();
        assert!(content.contains("codex --version"));
        assert!(content.contains("lb setup codex"));
        assert!(content.contains("trk setup codex"));
        assert!(content.contains("command -v codex >/dev/null"));
        assert!(content.contains("Codex state is persisted"));
    }

    #[test]
    fn setup_script_is_executable() {
        let dir = tempfile::tempdir().unwrap();
        write_setup_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            TEST_DOCKERFILE,
            TEST_DOCKERFILE_PATH,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(dir.path().join("setup.sh"))
                .unwrap()
                .permissions();
            assert_eq!(perms.mode() & 0o111, 0o111, "setup.sh should be executable");
        }
    }

    #[test]
    fn task_script_embeds_model_and_prompt() {
        let dir = tempfile::tempdir().unwrap();
        write_task_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            "do the thing",
        )
        .unwrap();
        let content = std::fs::read_to_string(dir.path().join("task.sh")).unwrap();
        assert!(content.contains("--model opus"));
        assert!(content.contains("'do the thing'"));
    }

    #[test]
    fn task_script_supports_codex_agent() {
        let dir = tempfile::tempdir().unwrap();
        write_task_script(
            crate::agent::AgentKind::Codex,
            dir.path(),
            "gpt-5.2",
            "do the thing",
        )
        .unwrap();
        let content = std::fs::read_to_string(dir.path().join("task.sh")).unwrap();
        assert!(content.contains("codex exec --json --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check --model gpt-5.2 'do the thing' </dev/null"));
        assert!(!content.contains("claude -p --dangerously-skip-permissions"));
    }

    #[test]
    fn task_script_escapes_single_quotes_in_prompt() {
        let dir = tempfile::tempdir().unwrap();
        write_task_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            "don't break",
        )
        .unwrap();
        let content = std::fs::read_to_string(dir.path().join("task.sh")).unwrap();
        assert!(content.contains(r#"'don'\''t break'"#));
    }

    #[test]
    fn task_script_switches_branch() {
        let dir = tempfile::tempdir().unwrap();
        write_task_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            "prompt",
        )
        .unwrap();
        let content = std::fs::read_to_string(dir.path().join("task.sh")).unwrap();
        assert!(content.contains("git fetch origin"));
        assert!(content.contains(r#"git checkout "$branch""#));
    }

    #[test]
    fn task_script_stashes_leftover_state() {
        let dir = tempfile::tempdir().unwrap();
        write_task_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            "prompt",
        )
        .unwrap();
        let content = std::fs::read_to_string(dir.path().join("task.sh")).unwrap();
        assert!(content.contains("git stash push"));
        assert!(content.contains("::mrmouth::warning uncommitted changes"));
    }

    #[test]
    fn task_script_overwrites_previous() {
        let dir = tempfile::tempdir().unwrap();
        write_task_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "opus",
            "first prompt",
        )
        .unwrap();
        write_task_script(
            crate::agent::AgentKind::Claude,
            dir.path(),
            "sonnet",
            "second prompt",
        )
        .unwrap();
        let content = std::fs::read_to_string(dir.path().join("task.sh")).unwrap();
        assert!(content.contains("--model sonnet"));
        assert!(content.contains("'second prompt'"));
        assert!(!content.contains("first prompt"));
    }
}
