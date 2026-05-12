# Log

## [2026-05-12] documented | Litebrite agent contract
Captured the litebrite command semantics that matter for mrmouth and supervising agents: `lb prime` as the intended AI context surface, the ready/show/claim/work/commit/close/sync protocol, claim atomicity, ready discovery semantics, close-with-open-children behavior, and implications for a future mrmouth operator skill.

## [2026-05-12] clarified | Brite hierarchy model
Recorded that mrmouth-prepared brites should primarily form a nested parent/child work breakdown. Parents roll up child completion rather than depending on children via `blocks`; explicit `blocks` dependencies should model sequencing between siblings.

## [2026-05-12] generalized | Brite authoring as work graph design
Captured that the desired Codex capability is broader than mrmouth operation: author nested litebrite work graphs from user goals, with parent brites for outcomes, leaf brites as ramp-in handoffs, and sibling `blocks` for true ordering constraints.

## [2026-05-12] planned | Event rendering separation
Created Litebrite epic `lb-40uv` for separating mrmouth lifecycle events from TUI rendering and recorded the architectural intent: core flows emit events, renderers handle TUI/human/JSON/log presentation, and lifecycle JSON remains distinct from raw inner-agent JSON.

## [2026-05-12] fixed | Dockerfile extraction after pull
Recorded the lifecycle rule for container-edited Dockerfiles: remote-backed runs should pull first, then extract via a temporary file and leave the host worktree untouched when the post-pull Dockerfile already matches the container content. This prevents self-produced, already-pushed Dockerfile edits from blocking `git pull --ff-only`.
