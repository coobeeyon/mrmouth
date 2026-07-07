use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::debrief::FailureDebrief;
use crate::do_cmd;
use crate::events::{
    BranchAction, EventSinkHandle, FinishStatus, LifecycleSummary, MessageLevel, MessageTarget,
    MrmouthEvent,
};
use crate::prompt;
use crate::repo_layout::RepoLayout;
use crate::run::{self, RunOptions};
use crate::tui::TuiHandle;

pub struct BatchOptions {
    pub item_id: String,
    pub worktree: Option<PathBuf>,
    pub repo_layout: Option<RepoLayout>,
    pub current_container: bool,
    pub max_items: u32,
    pub context_ceiling_percent: u32,
    pub timeout: u32,
    pub model: String,
    pub json_events: bool,
    pub event_sink: Option<EventSinkHandle>,
}

#[derive(Debug)]
pub enum BatchError {
    Command(String),
    Run(Box<run::RunError>),
}

impl BatchError {
    pub fn debrief(&self) -> FailureDebrief {
        match self {
            Self::Run(err) => {
                let mut d = err.debrief();
                d.message = format!("batch agent failed: {}", d.message);
                d
            }
            Self::Command(message) => FailureDebrief::new(message.clone()),
        }
    }
}

impl std::fmt::Display for BatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command(message) => write!(f, "{message}"),
            Self::Run(err) => write!(f, "batch agent failed: {err}"),
        }
    }
}

impl std::error::Error for BatchError {}

struct ParentInfo {
    title: String,
    item_type: String,
}

pub fn execute(
    config: &Config,
    repo_root: &Path,
    opts: BatchOptions,
    tui: Option<&TuiHandle>,
) -> Result<(), BatchError> {
    if !opts.current_container {
        return Err(BatchError::Command(
            "batch currently requires --current-container".to_string(),
        ));
    }
    let Some(worktree_path) = opts.worktree.clone() else {
        return Err(BatchError::Command(
            "batch current-container mode requires --worktree <path> or work_repo in .mrmouth/config.toml"
                .to_string(),
        ));
    };
    if opts.max_items == 0 {
        return Err(BatchError::Command(
            "--max-items must be greater than 0".to_string(),
        ));
    }

    emit_event(
        &opts.event_sink,
        MrmouthEvent::StageChanged {
            stage: "Batch".to_string(),
        },
    );

    let parent = lb_show(repo_root, &opts.item_id)?;
    emit_event(
        &opts.event_sink,
        MrmouthEvent::TaskSelected {
            item_id: opts.item_id.clone(),
            title: parent.title.clone(),
            parent_id: None,
        },
    );
    emit_event(
        &opts.event_sink,
        MrmouthEvent::TaskLabel {
            item_id: opts.item_id.clone(),
            name: "type".to_string(),
            value: parent.item_type.clone(),
        },
    );
    emit_event(
        &opts.event_sink,
        MrmouthEvent::TaskLabel {
            item_id: opts.item_id.clone(),
            name: "workspace".to_string(),
            value: worktree_path.display().to_string(),
        },
    );

    let current_branch =
        do_cmd::git_current_branch(repo_root).map_err(|e| BatchError::Command(e.to_string()))?;
    let feature_branch = if current_branch == "main" || current_branch == "master" {
        let branch = format!(
            "{}-batch-{}",
            opts.item_id,
            do_cmd::make_slug(&parent.title)
        );
        emit_event(
            &opts.event_sink,
            MrmouthEvent::BranchLifecycle {
                action: BranchAction::Creating,
                branch: branch.clone(),
                parent_branch: Some(current_branch.clone()),
            },
        );
        do_cmd::git_checkout_new_branch(repo_root, &branch)
            .map_err(|e| BatchError::Command(e.to_string()))?;
        emit_event(
            &opts.event_sink,
            MrmouthEvent::BranchLifecycle {
                action: BranchAction::Created,
                branch: branch.clone(),
                parent_branch: Some(current_branch),
            },
        );
        push_branch(repo_root, &branch, &opts.event_sink);
        branch
    } else {
        current_branch
    };

    let base_prompt = prompt::load_prompt(repo_root, None);
    let prompt = batch_prompt(
        repo_root,
        &worktree_path,
        &opts.item_id,
        opts.max_items,
        opts.context_ceiling_percent,
        &base_prompt,
    );

    let run_opts = RunOptions {
        raw: false,
        json_events: opts.json_events,
        emit_terminal_events: false,
        model: opts.model.clone(),
        timeout: Some(opts.timeout),
        local: false,
        current_container: true,
        local_workspace_path: None,
        worktree_path: Some(worktree_path.clone()),
        repo_layout: opts.repo_layout.clone(),
        prompt_override: Some(prompt),
        branch: None,
        event_sink: opts.event_sink.clone(),
    };

    match run::execute(config, repo_root, run_opts, tui) {
        Ok(ref run_logger) => {
            emit_info(&opts.event_sink, "Batch agent succeeded, syncing...");
            do_cmd::sync_and_push(repo_root, &feature_branch, Some(run_logger));
        }
        Err(err) => return Err(BatchError::Run(Box::new(err))),
    }

    emit_event(
        &opts.event_sink,
        MrmouthEvent::finished(FinishStatus::Success, None::<String>),
    );
    let summary = attach_latest_log_paths(
        repo_root,
        &config.log_dir,
        LifecycleSummary::success("batch")
            .item_id(opts.item_id)
            .branch(feature_branch)
            .workspace(worktree_path.display().to_string())
            .next_action("merge_when_ready"),
    );
    emit_event(&opts.event_sink, MrmouthEvent::LifecycleSummary { summary });
    Ok(())
}

fn batch_prompt(
    repo_root: &Path,
    worktree_path: &Path,
    item_id: &str,
    max_items: u32,
    context_ceiling_percent: u32,
    base_prompt: &str,
) -> String {
    format!(
        "## Scope\n\n\
        You are working on parent Litebrite item {item_id}. Run `lb show {item_id}` and \
        `lb list --parent {item_id}` in the bookkeeping repo to inspect the child tasks. \
        The litebrite/task tracking repo is `{}`; run `lb` commands there. \
        The code worktree to edit is `{}`. \
        Complete up to {max_items} ready/open child tasks in dependency order in this single runner execution. \
        For each child task: run `lb show <child>`, claim it, implement only that child's requested scope, \
        run its requested verification, commit code changes in the code worktree, close that child, and sync task state. \
        Keep separate commits for separate child tasks. Do not edit tests or data unless a child explicitly asks for it. \
        Stop before the conversation feels near {context_ceiling_percent}% of the model context; if that happens, \
        finish the current child cleanly and leave remaining children open. \
        If all children are complete, close parent item {item_id}. \
        Leave both repositories clean except ignored eval artifacts, then push.\n\n\
        {base_prompt}",
        repo_root.display(),
        worktree_path.display(),
    )
}

fn lb_show(repo_root: &Path, item_id: &str) -> Result<ParentInfo, BatchError> {
    let output = Command::new("lb")
        .args(["show", item_id])
        .current_dir(repo_root)
        .output()
        .map_err(|e| BatchError::Command(format!("failed to run lb show: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BatchError::Command(format!(
            "Item {item_id} not found: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let title = field_from_lb_show(&stdout, "Title").unwrap_or_else(|| item_id.to_string());
    let item_type = field_from_lb_show(&stdout, "Type").unwrap_or_else(|| "task".to_string());
    Ok(ParentInfo { title, item_type })
}

fn field_from_lb_show(stdout: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}:");
    stdout
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))
        .map(|line| line.trim().trim_start_matches(&prefix).trim().to_string())
}

fn push_branch(repo_root: &Path, branch: &str, sink: &Option<EventSinkHandle>) {
    emit_event(
        sink,
        MrmouthEvent::BranchLifecycle {
            action: BranchAction::Pushing,
            branch: branch.to_string(),
            parent_branch: None,
        },
    );
    let pushed = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "push",
            "-u",
            "origin",
            branch,
        ])
        .output()
        .is_ok_and(|output| output.status.success());
    if pushed {
        emit_event(
            sink,
            MrmouthEvent::BranchLifecycle {
                action: BranchAction::Pushed,
                branch: branch.to_string(),
                parent_branch: None,
            },
        );
    } else {
        emit_info(
            sink,
            "Warning: git push failed; current-container batch may still continue locally.",
        );
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

fn emit_info(sink: &Option<EventSinkHandle>, text: &str) {
    emit_event(
        sink,
        MrmouthEvent::Message {
            level: MessageLevel::Info,
            text: text.to_string(),
            target: MessageTarget::Agent,
        },
    );
}

fn emit_event(sink: &Option<EventSinkHandle>, event: MrmouthEvent) {
    if let Some(sink) = sink {
        sink.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_prompt_names_parent_limits_and_worktree() {
        let prompt = batch_prompt(
            Path::new("/repo"),
            Path::new("/repo/worktree"),
            "lb-1234",
            3,
            50,
            "base",
        );

        assert!(prompt.contains("parent Litebrite item lb-1234"));
        assert!(prompt.contains("Complete up to 3 ready/open child tasks"));
        assert!(prompt.contains("near 50% of the model context"));
        assert!(prompt.contains("The code worktree to edit is `/repo/worktree`"));
        assert!(prompt.contains("base"));
    }

    #[test]
    fn parses_lb_show_fields() {
        let stdout = "  ID: lb-1\n  Title: Demo Parent\n  Type: epic\n";
        assert_eq!(
            field_from_lb_show(stdout, "Title"),
            Some("Demo Parent".to_string())
        );
        assert_eq!(field_from_lb_show(stdout, "Type"), Some("epic".to_string()));
    }
}
