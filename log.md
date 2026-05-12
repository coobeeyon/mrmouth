# Log

## [2026-05-12] documented | Litebrite agent contract
Captured the litebrite command semantics that matter for mrmouth and supervising agents: `lb prime` as the intended AI context surface, the ready/show/claim/work/commit/close/sync protocol, claim atomicity, ready discovery semantics, close-with-open-children behavior, and implications for a future mrmouth operator skill.

## [2026-05-12] clarified | Brite hierarchy model
Recorded that mrmouth-prepared brites should primarily form a nested parent/child work breakdown. Parents roll up child completion rather than depending on children via `blocks`; explicit `blocks` dependencies should model sequencing between siblings.
