use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Claude,
    Codex,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn binary(self) -> &'static str {
        self.as_str()
    }

    pub fn home_mount(self) -> &'static str {
        match self {
            Self::Claude => "/home/runner/.claude",
            Self::Codex => "/home/runner/.codex",
        }
    }

    pub fn default_volume_prefix(self) -> &'static str {
        match self {
            Self::Claude => "mrmouth-claude-home",
            Self::Codex => "mrmouth-codex-home",
        }
    }

    pub fn version_line(self) -> String {
        let bin = self.binary();
        format!("{bin} --version 2>/dev/null || echo \"{bin}: not installed\"")
    }

    pub fn restore_block(self) -> &'static str {
        match self {
            Self::Claude => {
                r#"# --- Restore .claude.json from persisted backup if missing ---
claude_config="$HOME/.claude.json"
if [ ! -f "$claude_config" ] && [ -d "$HOME/.claude/backups" ]; then
  latest_backup=$(ls -t "$HOME/.claude/backups/.claude.json.backup."* 2>/dev/null | head -1 || true)
  if [ -n "$latest_backup" ]; then
    cp "$latest_backup" "$claude_config"
    echo "Restored .claude.json from backup: $(basename "$latest_backup")"
  fi
fi"#
            }
            Self::Codex => {
                r#"# --- Codex state is persisted in $HOME/.codex by the Docker volume ---"#
            }
        }
    }

    pub fn shell_command(
        self,
        model: &str,
        escaped_prompt: &str,
        escaped_schema: Option<&str>,
    ) -> String {
        match self {
            Self::Claude => match escaped_schema {
                Some(schema) => format!(
                    "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING=1 claude -p --dangerously-skip-permissions --verbose --output-format stream-json --model {model} --effort xhigh --json-schema '{schema}' '{escaped_prompt}'"
                ),
                None => format!(
                    "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING=1 claude -p --dangerously-skip-permissions --verbose --output-format stream-json --disallowedTools EnterPlanMode,ExitPlanMode --model {model} --effort xhigh '{escaped_prompt}'"
                ),
            },
            Self::Codex => {
                let model_arg = codex_model_arg(model);
                match escaped_schema {
                    Some(schema) => format!(
                        "cat > /tmp/mrmouth-output-schema.json << 'MRMOUTH_SCHEMA_EOF'\n{schema}\nMRMOUTH_SCHEMA_EOF\ncodex exec --json --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check --output-schema /tmp/mrmouth-output-schema.json{model_arg} '{escaped_prompt}' </dev/null"
                    ),
                    None => format!(
                        "codex exec --json --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check{model_arg} '{escaped_prompt}' </dev/null"
                    ),
                }
            }
        }
    }

    pub fn shell_command_with_disallowed_tools(self, model: &str, escaped_prompt: &str) -> String {
        match self {
            Self::Claude => self.shell_command(model, escaped_prompt, None),
            Self::Codex => self.shell_command(model, escaped_prompt, None),
        }
    }
}

fn codex_model_arg(model: &str) -> String {
    if model.trim().is_empty() {
        String::new()
    } else {
        format!(" --model {model}")
    }
}
