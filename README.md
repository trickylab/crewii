# crewii

[![CI](https://github.com/trickylab/crewii/actions/workflows/ci.yml/badge.svg)](https://github.com/trickylab/crewii/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![OpenCode](https://img.shields.io/badge/OpenCode-%E2%89%A5%202.2.5-black.svg)](https://opencode.ai)

A role-based engineering crew for [OpenCode](https://opencode.ai). One command opens a tab per role. You talk to a business analyst, it writes a brief, and role leads split the work across parallel workers. Each lead verifies the result before it reports back.

```sh
crewii eng
crewii ask eng "Fix the login error"
```

Crewii is a single Rust binary with an embedded OpenCode plugin. The generated permission rules never use `ask`, so a crew runs without permission prompts unless your own OpenCode configuration requires them. Crewii is not affiliated with OpenCode.

## Contents

- [Requirements](#requirements)
- [Install](#install)
- [Quick start](#quick-start)
- [How it works](#how-it-works)
- [Roles](#roles)
- [Permissions](#permissions)
- [Writer output](#writer-output)
- [Configuration](#configuration)
- [Files and isolation](#files-and-isolation)
- [Commands](#commands)
- [Remove](#remove)
- [Troubleshooting](#troubleshooting)
- [Credits](#credits)

## Requirements

- OpenCode 2.2.5 or newer, with a provider and default model configured.
- Git, plus network access on the first run to fetch the pinned personas.
- macOS or Linux. On Windows, use WSL; native Windows support is planned.

Crewii does not need Node or Python at runtime. Writer worktrees need a Git repository, and a project `AGENTS.md` helps every role.

## Install

With Homebrew on macOS or Linux:

```sh
brew install trickylab/tap/crewii
```

Or download the archive for your platform and its `.sha256` file from [Releases](https://github.com/trickylab/crewii/releases). Builds cover macOS and Linux (musl), each on arm64 and x86_64. Verify the checksum, then put the extracted `crewii` binary on your `PATH`:

```sh
shasum -a 256 -c crewii-v0.1.0-aarch64-apple-darwin.tar.gz.sha256
```

Replace the filename with the one for your release and platform. With a Rust toolchain you can build from source instead:

```sh
cargo install --git https://github.com/trickylab/crewii
```

## Quick start

> [!WARNING]
> The [permission rules](#permissions) are guardrails, not a sandbox. Agents can read files outside the project and environment variables; the floor blocks named `.env*` files and home secret locations, with the exceptions and limits described below. A crew runs many model sessions in parallel, so token use and provider costs grow with it.

From your project directory:

```sh
crewii doctor
crewii engineering
```

The first interactive open asks for two models: a primary model for leads and a subagent model for workers. Press Enter to keep the displayed default. Non-interactive opens use the saved choices without asking. To change them later:

```sh
crewii models
crewii models --primary provider/model --subagent provider/model --global
```

Reopen the crew to apply new choices.

Talk to `ba-01` in its tab. The BA clarifies the request and routes a brief to the right role. You can also send a request from another terminal in the same project, once the crew is open:

```sh
crewii ask eng "Review the latest changes"
```

The command prints `sent to the engineering crew (see the ba tab)`. The ticket ID, any follow-up questions, and the results appear in the BA tab. The request text travels as a command argument and can show up in the process list, so keep secrets out of it.

For a code change, inspect the branch the BA reports:

```sh
crewii status
git branch --list 'crewii/*'
git log main..crewii/engineering-T-1-example
```

Use your own base branch and the reported branch name. Review and merge the work yourself, then run `crewii rm engineering` when you are done.

`crewii doctor` and `crewii check eng` exit with status 1 on failure. `check` inspects isolation and permissions for a crew that is already built. The first `check` or open that loads the plugin creates one lead session per role.

## How it works

```text
you -> BA -> role lead -> workers (lead waits)
        <- report  <- results
```

The BA writes briefs and routes tickets. A lead plans and verifies but never edits: it reads the brief, splits it into sub-tasks, and runs up to `workersPerTicket` workers. Read-only sub-tasks and sub-tasks in different repositories run in parallel. When every worker has finished, the lead checks the results against the acceptance criteria and sends one report back. Busy roles get extra numbered instances, and their tabs are grouped by role. Use `/crew` inside OpenCode to see instances and the backlog, or to jump to a tab.

### Tickets

Only you open BA tickets, and only the BA opens root tickets. Other roles reply or open linked sub-tickets. Delegation is limited to four levels and cycles are rejected.

Work stays inside the brief and its acceptance criteria. Existing problems that do not block the ticket go under Notes rather than becoming fixes or new tickets. Every result uses the brief's Report format, and final reports are saved to `.opencode/crewii/reports/<division>-T-n.md`.

A requester can steer a ticket while the holder works on it. Replying to the holder on a closed ticket reopens it; a reply from the holder to the requester does not, and a second final reply to a closed ticket is an error. The BA can cancel tickets it holds, and any requester can cancel their own tickets with `crew_cancel`.

### Dispatching

New work goes to an idle instance, or to the backlog when every instance is busy. Backlog tickets start automatically as capacity frees up. Mark a request as urgent and the BA sets `urgent: true`, which moves the ticket ahead in the backlog. A ticket that names a locked worktree goes to the instance writing in it.

Inactive work can be reassigned after `staleMinutes` (default 15, `0` disables takeover). BA holders, tickets with open sub-tickets, and replies waiting on the requester are never taken over. The new holder gets the previous holder's last update.

`crewii status` lists instances with their last activity, the backlog, recent cancellations, ticket cycle times, and worktrees. Leads use the `crew_status` tool to check progress without opening status tickets.

### Workers

Workers run the tests for the packages they change; leads run the full suite once when permissions allow. Leads size each sub-task to about `workerSteps` steps (default 60). This is sizing guidance and not a hard cap, because OpenCode's step cap currently fails on models that reject assistant prefill. A lead can resume a worker that stops early.

When a denied command fails with `Permission denied: shell`, OpenCode does not say which rule matched. Leads and workers are told to run the parts of the command one at a time to find the blocked one.

### Recovery

When an instance's last open ticket closes, is cancelled, or is reassigned, the plugin resets its context without a model call. If a session is lost, its tickets close and their requesters are notified. Corrupt state is backed up before recovery. A state file from a newer plugin version is refused and left intact.

## Roles

| Role | Purpose | Profile |
| --- | --- | --- |
| ba | Requirements, acceptance criteria, routing | coordinator |
| architect | Cross-component design and documentation | docs-writer |
| backend | Server-side implementation | writer |
| frontend | User-facing implementation | writer |
| reviewer | Correctness and maintainability review | read-only |
| qa | Tests and acceptance criteria | writer |
| security | Security and trust-boundary review | read-only |

## Permissions

Crewii layers its own rules on top of your OpenCode configuration and never adds a blanket allow. Tools crewii does not mention, such as skills and webfetch, follow your configuration. Generated rules use only `allow` or `deny`; inherited rules can still prompt.

Leads cannot edit files or run Git commands that change state, whatever their role, profile, or overrides say. Profiles and role overrides apply to workers:

| Profile | Edits | State-changing Git |
| --- | --- | --- |
| coordinator | `*docs/*.md` only | denied |
| docs-writer | `*docs/*.md` only | allowed in its worktree |
| read-only | denied | denied |
| writer | your configuration | your configuration |

Every role shares a floor of deny rules:

- `git push` and `git merge`, with or without Git options.
- `sudo`, `kubectl`, and `gh` writes: creating, editing, or deleting repositories, releases, pull requests, and issues; `gh workflow run`; and `gh api` with a method or fields. Reads such as `gh repo view`, `gh release list`, and `gh api <path>` are allowed.
- Reads of `.env` and `.env.*`, except example, sample, and template files.
- Named paths under `~/.ssh`, `~/.aws`, and `~/.config/gh`, plus `~/.netrc`.

Other shell commands follow your configuration. Role overrides are evaluated before the floor, so an override can only weaken it by removing a floor rule explicitly.

> [!CAUTION]
> Shell matching is pattern-based. OpenCode splits compound commands, but wrappers and aliases such as `sh -c`, `env`, and Git aliases can slip past it, and so can redirects without a space (`cmd>file`). Shell rules do not cover other tools: a read-only role can still call a write tool if your configuration allows it. Close that gap with a role-specific deny in the project override, using the real tool name:
>
> ```json
> {"roles":[{"id":"reviewer","permissions":[{"action":"<tool-name>","resource":"*","effect":"deny"}]}]}
> ```
Where Git state changes are denied, only the writing forms are blocked. Listings such as `git tag --sort=-v:refname`, `git branch -a --list 'x*'`, `git remote get-url`, and `git config --get` stay allowed.


## Writer output

Writer roles (backend, frontend, and qa) are told to work in `<root>/.opencode/crewii/worktrees/<division>-<ticket>` on a local branch named `crewii/<division>-<ticket>-<slug>`, commit there, and report the worktree, branch, and commits. They should leave the primary checkout alone. Crewii never pushes branches, so you review and integrate them.

The edit tool locks each worktree to one instance with an open ticket. Shell writes and code generators bypass that lock. Both the worktree rule and the lock are agent instructions backed by guardrails, not enforced isolation.

Workers keep temporary files in `<root>/.opencode/crewii/tmp/<division>-<ticket>/`, which is removed when the ticket closes. `crewii status` reports worktree and temporary storage sizes. `crewii status --prune` offers to remove leftover temporary directories and clean worktrees of closed tickets, keeping their branches. Crewii never deletes a worktree on its own, since it may hold uncommitted work.

OpenCode matches edit paths relative to the crew session directory, `<root>/.opencode/crewii/<division>`, so a crew worktree file arrives as `../worktrees/<name>/<file>`. Crewii adds that relative form for every non-shell override rule that contains `.opencode/crewii/`, so `*/.opencode/crewii/worktrees/*` works as written.

## Configuration

Create a project override:

```sh
crewii init engineering
```

Then edit `<root>/.opencode/crewii/engineering.json`:

```json
{
  "limits": {"instancesPerRole": 2},
  "roles": [{"id": "reviewer", "persona": "engineering/engineering-code-reviewer.md"}],
  "floor": {"remove": ["gh api* -X*"]}
}
```

| Key | Meaning |
| --- | --- |
| `limits.instancesPerRole` | Maximum instances per role. |
| `limits.workersPerTicket` | Maximum workers a lead runs for one ticket. |
| `limits.ticketsPerBa` | Maximum open tickets per BA instance. |
| `limits.workerSteps` | Sub-task size target for leads. Positive, default 60. |
| `limits.staleMinutes` | Minutes before takeover. Zero or positive, default 15; `0` disables takeover. |
| `floor.remove` | Floor rules to drop. |
| `roles[].persona`, `boundary`, `description`, `permissions` | Overrides for existing roles. |

A `persona` is a relative path inside agency-agents at the pinned commit, without an absolute path or a `..` component. Crewii reads the file from that commit rather than from the cache worktree and strips its frontmatter.

Remove floor rules only when you need to. A removed rule does not grant access by itself; the remaining rules still apply.

Model choices are stored per directory in `<root>/.opencode/crewii/state/models.json` and globally in `~/.config/crewii/config.json`. Directory values win field by field, and unset fields fall back to the OpenCode default. An absolute `XDG_CONFIG_HOME` moves the global file to `$XDG_CONFIG_HOME/crewii/`; relative `XDG_CONFIG_HOME` and `XDG_CACHE_HOME` values are ignored.

Reopen the division after any change to regenerate its files.

## Files and isolation

The project root is the Git top-level directory, or the current directory outside a repository.

| Path | Contents |
| --- | --- |
| `<root>/.opencode/crewii/engineering/` | Generated agents and plugin, with persona attribution and a `LICENSE.agency-agents` copy. |
| `<root>/.opencode/crewii/state/engineering.json` | Session IDs and tickets, mode `0600`. Ticket titles are the first 60 characters of the whitespace-normalized request. |
| `<root>/.opencode/crewii/engineering.json` | Your override. Safe to commit. |
| `$XDG_CACHE_HOME/crewii/agency-agents` | Persona cache, `~/.cache/crewii/agency-agents` by default. |

Crewii launches OpenCode in the division directory with `OPENCODE_CLI_CONFIG_CONTENT` setting tabs to `on` and scope to `cwd`. It does not write your `cli.json` or tab files. Project instructions and your OpenCode configuration stay shared with the crew.

In a Git repository, crewii adds one line, `/.opencode/crewii/*/`, to `.git/info/exclude` and leaves `.gitignore` alone. Generated directories stay out of Git while the JSON override can still be committed.

## Commands

| Command | Description |
| --- | --- |
| `crewii <division>` | Build and open a crew (`eng` is an alias for `engineering`). Same as `crewii open <division>`. |
| `crewii ask <division> <text>` | Send a request to the BA. Quotes are optional. |
| `crewii status [division] [--prune]` | Show instances, backlog, cycle times, worktrees, and storage. |
| `crewii models` | Choose primary and subagent models. |
| `crewii list` | List crews in this project. |
| `crewii init <division>` | Create a project override. |
| `crewii doctor` | Check requirements and configuration. |
| `crewii check [division]` | Check crew isolation and permissions. |
| `crewii rm <division> [--yes]` | Remove a crew and its sessions. |

Run `crewii` with no arguments to list divisions, or `crewii help <command>` for details. Set `CREWII_OPENCODE` to point crewii at a specific OpenCode executable.

## Remove

```sh
crewii rm engineering
```

The confirmation defaults to No; `--yes` skips only the confirmation. Removal deletes the crew's sessions, clean worktrees, generated files, reports, and state. Branches and your override are kept, and dirty or detached worktrees are kept and reported. OpenCode keeps the old tabs in its tab list; the next `crewii <division>` closes the ones that belong to removed crew sessions.

To uninstall, delete the binary and the crewii cache, plus any project overrides you no longer need.

## Troubleshooting

- Run `crewii doctor` first. If OpenCode is not on your `PATH`, set `CREWII_OPENCODE` to an executable path or command name.
- If `crewii check eng` reports an `ask` permission, add an explicit allow or deny for that tool and resource in the project override, then reopen the crew.
- If the plugin is not loaded, rebuild and open the crew with `crewii engineering`.
- Run one OpenCode server per project. State updates are serialized only within a single server.
- On Windows, use WSL. Native Windows needs a separate read-only shell profile and is not supported yet.

## Credits

Personas come from [agency-agents](https://github.com/msitarzewski/agency-agents) under the MIT license. See [NOTICE](NOTICE).

## License

[MIT](LICENSE)
