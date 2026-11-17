Role-based engineering crew for OpenCode: one tab per role, lead + parallel workers, auto-scaled instances, no generated permission prompts.

Not affiliated with OpenCode.

Generated permission rules never use `ask`; your OpenCode configuration can still require prompts for tools crewii does not control.

## Requirements

- OpenCode 2.2.5 or newer, with a provider and default model configured.
- Git and network access on first run to fetch the pinned personas.
- macOS, Linux, or Windows through WSL. Native Windows support is planned.

Node and Python are not required to run crewii. A Git repository and project `AGENTS.md` are recommended; writer worktrees require a Git repository.

## Install

Download the archive and its `.sha256` file from [Releases](https://github.com/trickylab/crewii/releases). Archives cover Linux musl and macOS, both arm64 and x86_64. In the download directory, verify the matching checksum and put the extracted `crewii` binary on your PATH:

```sh
shasum -a 256 -c crewii-v0.1.0-aarch64-apple-darwin.tar.gz.sha256
```

Use the filename for your release and platform. Alternatively, with Rust installed:

```sh
cargo install --git https://github.com/trickylab/crewii
```

Homebrew support is planned.

## Before you start

The [permission guardrails](#permissions) are not a sandbox. Agents can read files outside the project and environment variables; the floor restricts named `.env*` files and home secret locations, with the exceptions and limits below. A crew can run many model sessions in parallel, increasing token use and provider costs.

## Quick start

In your project directory:

```sh
crewii doctor
crewii engineering
```

On first interactive open, choose a primary model for leads and a subagent model for workers. Enter keeps the displayed default. Non-interactive opens use saved defaults without prompting. Run `crewii models` to change choices, or `crewii models --primary provider/model --subagent provider/model --global` to set global defaults. Reopen the crew to apply changes.

Talk to `ba-01` in its tab. The BA clarifies the request and routes a brief to the crew. From another terminal in the project, you can also send a request:

```sh
crewii ask eng "Review the latest changes"
```

The command prints `sent to the engineering crew (see the ba tab)`; the ticket ID, questions, and results appear in the BA tab. Open the crew first so its sessions exist. Request text is passed as a command argument and can be visible in the process list; do not include secrets.

For a code change, ask the crew, then inspect the branch reported in the BA tab:

```sh
crewii ask eng "Fix the login error"
crewii status
git branch --list 'crewii/*'
git log main..crewii/engineering-T-1-example
```

Replace `main` and the example branch with your base and the reported branch. Review the changes and merge them yourself, then run `crewii rm engineering` when finished.

```sh
crewii status eng
crewii check eng
```

`doctor` and `check` exit with status 1 on failure. `check` inspects isolation and permissions for an already-built crew. The first check or open that loads the division plugin creates one lead session per role.

## How it works

```text
request -> BA -> role lead -> workers (lead waits)
              <- report    <- results
```

The BA creates briefs and routes tickets. A role lead does not do the work itself: it analyses the brief, splits it into sub-tasks, runs up to `workersPerTicket` workers (in parallel when they are read-only or touch different repositories), waits for all of them, verifies the results against the acceptance criteria, and sends one report back. Busy roles can get additional numbered instances, with tabs grouped by role. Tickets and instances are stored in the project; `crewii status` shows instances, backlog, recent cancellations, and worktrees. Use `/crew` in OpenCode to view instances and backlog tickets or open a tab.

Requester replies steer the holder while it works; tickets naming a locked worktree route to its writer. Final reports are saved at `.opencode/crewii/reports/<division>-T-n.md` and removed by `crewii rm`. Status shows heartbeat activity and stale holders; leads and workers are told how to read "Permission denied: shell" (OpenCode does not report which rule denied a command). BA may cancel tickets it holds.

Only the user opens BA tickets; only BA opens root tickets. Other roles reply or open linked sub-tickets. After an instance's last open ticket closes, is cancelled, or is reassigned, the plugin resets its context without a model call. Results use the BRIEF's Report format.

`crewii status` shows recent ticket cycle times; set `urgent: true` on a new ticket when the user marks the request urgent to prioritize it in the backlog.

The dispatcher assigns new work to an idle instance or places it in the backlog when capacity is full, then dispatches it automatically as capacity frees. Requesters can cancel their tickets with `crew_cancel`. Inactive work may be reassigned after `staleMinutes` (default 15; 0 disables takeover), with exceptions for BA holders, open sub-tickets, and replies awaiting the requester.

Leads use `crew_status` to check progress without opening status tickets. A requester replying to the holder on a closed ticket reopens it; a holder's reply to the requester does not. A final reply to a closed ticket remains an error. Work stays within the BRIEF and its acceptance criteria; non-blocking existing issues go under Notes, not into fixes or new tickets. Workers run tests for changed packages; leads run the full suite once when permissions allow. Leads size worker sub-tasks to about `workerSteps` steps; this is guidance, not a hard cap (OpenCode's step cap currently fails on models without assistant prefill). The lead can resume a worker that stops early.

Delegation is limited to four levels; cycles are rejected. If a session is lost, its tickets are closed and their requesters notified. Corrupt state is backed up before recovery; a state version newer than the plugin supports is refused, not wiped.

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

Crewii adds its own rules on top of your OpenCode configuration; it does not add a blanket allow. Your configuration governs tools crewii does not mention, including skills and webfetch. Generated rules use only allow or deny, but inherited rules can still prompt.

Leads cannot edit files or run state-changing Git commands, whatever the role's access or overrides; the access profile and role overrides apply to workers.

- **coordinator:** edits limited to `*docs/*.md`; state-changing Git commands denied.
- **docs-writer:** edits limited to `*docs/*.md`; state-changing Git commands denied.
- **read-only:** edits and state-changing Git commands denied.
- **writer:** no additional profile restrictions; user configuration still applies.

Shell commands otherwise follow your configuration. The common floor denies Git push and merge with or without Git options, `sudo`, a fixed list of `gh` write subcommands, and `kubectl`. It denies reads of `.env` and `.env.*` files, with exceptions for example, sample, and template files. It also denies named paths under `~/.ssh`, `~/.aws`, and `~/.config/gh`, and `~/.netrc`. Role overrides come before the floor, so an override cannot bypass it without explicitly removing a floor rule.

**Guardrails, not a sandbox.** Shell matching is pattern-based. OpenCode's scanner splits compound commands, but wrappers and aliases such as `sh -c`, `env`, and Git aliases can bypass matching; so can redirects without a space, such as `cmd>file`. Shell rules do not cover other tools. A read-only role can invoke a write tool if your configuration allows it. Add a role-specific deny to the project override, replacing the placeholder with the actual tool name:

```json
{"roles":[{"id":"reviewer","permissions":[{"action":"<tool-name>","resource":"*","effect":"deny"}]}]}
```

## Writer output

Writer roles (backend, frontend, and qa) are instructed to work in `<root>/.opencode/crewii/worktrees/<division>-<ticket>` on a local branch named `crewii/<division>-<ticket>-<slug>`, commit there, and report the worktree, branch, and commits. They should not edit the primary checkout. Branches are never pushed by crewii; review and integrate them yourself. These are agent instructions and permission guardrails, not enforced isolation.

The edit tool locks each repository worktree to one crew instance with an open ticket. Shell writes and generators bypass this lock; it is not isolation.

### Temporary files and storage

Workers use `<root>/.opencode/crewii/tmp/<division>-<ticket>/` for temporary files. Ticket closure removes the directory automatically. `crewii status` reports worktree and temporary storage sizes; `crewii status --prune` offers to remove temporary directories for closed or cancelled tickets. Worktrees are never automatically deleted because they may hold uncommitted work; pruning clean worktrees requires confirmation.

## Customize

```sh
crewii init engineering
```

Edit `<root>/.opencode/crewii/engineering.json`. Overrides can set `limits` (`instancesPerRole`, `workersPerTicket`, `ticketsPerBa`, `workerSteps`, `staleMinutes`), `floor.remove`, and existing roles' `persona`, `boundary`, `description`, or `permissions`. `workerSteps` defaults to 60 and must be positive; `staleMinutes` accepts zero or a positive integer. For example:

```json
{
  "limits": {"instancesPerRole": 2},
  "roles": [{"id": "reviewer", "persona": "engineering/engineering-code-reviewer.md"}],
  "floor": {"remove": ["gh api*"]}
}
```

The `persona` value is a relative path inside agency-agents at the pinned commit, with no absolute path or `..` component. The file is read from that commit, not the cache worktree; its frontmatter is stripped.

Removing a floor rule weakens the guardrails; do it only when needed. A removed rule does not itself grant access: other rules still apply. Directory model choices live in `<root>/.opencode/crewii/state/models.json`; global defaults live in `~/.config/crewii/config.json`. Directory values take precedence per field; unset values use the OpenCode default. With an absolute `XDG_CONFIG_HOME`, the global config lives under `$XDG_CONFIG_HOME/crewii/`. Relative `XDG_CONFIG_HOME` and `XDG_CACHE_HOME` values are ignored.

Reopen the division to regenerate its files after changes.

## Isolation

Generated agents and plugins live only in the division directory at `<root>/.opencode/crewii/engineering/`. Agent files include persona attribution and a nearby `LICENSE.agency-agents` copy. The state file at `<root>/.opencode/crewii/state/engineering.json` is written with mode `0600`; it contains session IDs and ticket titles made from the first 60 characters of whitespace-normalized requests. Crewii launches OpenCode there with `OPENCODE_CLI_CONFIG_CONTENT` setting tabs to `on` and scope to `cwd`; it does not write the user's `cli.json` or tab files. Project instructions and the user's OpenCode configuration remain shared. The root is the Git top-level directory, or the current directory outside a Git repository.

In a Git repository crewii adds the single line `/.opencode/crewii/*/` to `.git/info/exclude`, not `.gitignore`. This excludes generated directories while leaving the JSON override available to commit. Persona files are cached under `$XDG_CACHE_HOME/crewii/agency-agents`, or `~/.cache/crewii/agency-agents` by default.

## Remove

```sh
crewii rm engineering
```

Confirmation defaults to No. Removal deletes crew sessions, clean worktrees, generated files, and state; branches remain. Dirty or detached worktrees are kept and reported. The override is kept. Tabs of removed sessions can stay in OpenCode's tab list; close them manually. `crewii rm --yes engineering` skips only the confirmation. `crewii status --prune engineering` offers to remove clean worktrees for closed tickets, keeping their branches.

To uninstall, remove the installed binary and the crewii cache. Remove project overrides separately if no longer needed.

## Troubleshooting

- Run `crewii doctor` to check requirements and configuration. Set `CREWII_OPENCODE` to an executable path or command name if needed.
- If `crewii check eng` reports an `ask` permission, add an explicit allow or deny rule for that tool and resource in the project override, then reopen the crew.
- If the plugin is not loaded, rebuild and open the crew with `crewii engineering`.
- If tabs remain after `crewii rm`, close them manually.
- Use one OpenCode server per project; state updates are serialized only within one server.

## Windows

Use WSL for now. Native Windows is not supported yet; its shell needs a separate read-only permission profile.

## Credits

Personas are fetched from [agency-agents](https://github.com/msitarzewski/agency-agents), licensed under MIT. See [NOTICE](NOTICE).
