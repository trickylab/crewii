# Changelog

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.1] - 2026-11-21

### Changed

- Permissions block only the writing forms of Git and `gh`. Read-only roles and leads can list tags, branches, remotes, and config with any options, and every role can read with `gh` (`gh repo view`, `gh release list`, `gh api <path>`).
- The architect worker creates its own worktree and commits its documents.
- Leads keep crew worktrees under `.opencode/crewii/worktrees/` even when project instructions name another location.

### Fixed

- Override rules for crew worktrees, such as `*/.opencode/crewii/worktrees/*`, now allow edits there. OpenCode compares paths relative to the crew session directory, so these files arrived as `../worktrees/...` and every edit was denied.
- The model picker accepts a full model ID with a variant, such as `provider/model#high`, and a search with a variant, such as `opus#high`.
- Reopening a crew after `crewii rm` closes the restored tabs of the removed sessions instead of showing a second set of role tabs.

## [0.1.0] - 2026-11-18

### Added

- Release jobs that update the Homebrew tap and publish to crates.io.
- Ticket cycle times in status and urgent backlog priority.
- Temporary file cleanup and storage usage reporting.
- Dispatcher backlog, automatic assignment, ticket cancellation, and configurable stale takeover.
- Worker sub-task sizing, BRIEF-scoped work and notes, progress checks, worktree edit locks, and closed-ticket replies.
- Engineering crew with seven roles, parallel sessions, ticket routing, and automatic scaling.
- Project-scoped tabs, writer worktrees and branches, primary and subagent model selection, and role overrides.
- Leads follow analyse, plan, dispatch, verify, report: they wait for their workers and return one verified report.
- Status, diagnostics, permission checks, and crew removal commands.
- Pinned upstream personas, attribution, and an upstream license copy in generated files.
- Linux musl and macOS arm64 and x86_64 release archives; Windows through WSL.
- Corrupt state backup, lost-session ticket recovery, and rejection of newer state versions.

### Changed

- Added steer replies, worktree-sticky routing, saved reports, activity status, permission-denied guidance, and BA cancellation of held tickets.

- Leads cannot edit; root tickets come only from BA; lead context resets after each finished ticket; acceptance criteria need checks and results use one Report format.
- Relaxed inspection permissions while retaining Git and secret guardrails; allow environment examples.

### Security

- Permission profiles and a common floor restrict shell commands and access to named secret files; these guardrails are not a sandbox.
- State files are written with mode `0600`.
- Delegation depth is limited and cycles are rejected.
