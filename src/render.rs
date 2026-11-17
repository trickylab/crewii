//! Agent markdown rendering (frontmatter and prompt).

use crate::{
    Result,
    perm::{Access, Effect, Kind, Rule, agent_rules},
    preset::Division,
};

#[derive(Debug)]
pub struct Rendered {
    pub leads: Vec<(String, String)>,
    pub workers: Vec<(String, String)>,
}

pub fn render(
    d: &Division,
    primary: Option<&str>,
    subagent: Option<&str>,
    persona: &dyn Fn(&str) -> Result<String>,
) -> Result<Rendered> {
    let mut rendered = Rendered {
        leads: Vec::new(),
        workers: Vec::new(),
    };
    for role in &d.roles {
        let body = persona(&role.persona)?;
        let teammates = d
            .roles
            .iter()
            .filter(|r| r.id != role.id)
            .map(|r| format!("- {}: {}", r.id, r.description))
            .collect::<Vec<_>>()
            .join("\n");
        let workers = if role.id == "ba" {
            "You do not start workers; send BRIEFs to role leads.".to_owned()
        } else {
            format!(
                "You plan and verify; workers do the work. You cannot edit files or change Git state; workers make every change, including git add and commit. For every ticket:\n1. Analyse: read the BRIEF and the relevant code until you know what must change.\n2. Plan: split the work into sub-tasks and decide how many workers to run, at most {n} per ticket. Size each sub-task so a worker can finish it in about {steps} steps; split larger work. Read-only sub-tasks may run in parallel; writer sub-tasks on the same repository run one after another.\n3. Dispatch: start each worker with the subagent tool (agent {d}/worker/{r}, background false) and wait for it. To run workers in parallel, call the subagent tool several times in the same message. In every worker prompt, include the ticket id, a short kebab-case slug, the complete BRIEF, and that worker's sub-task with its own acceptance criteria. Writing workers create their own worktree and branch (`crewii/{d}-<ticket>-<slug>` under `<root>/.opencode/crewii/worktrees/`); do not name branches or tell them to create or switch branches.\n4. Verify: when every worker has finished, verify each result yourself inside the worker's worktree; never check out or switch branches in the primary checkout. Run each acceptance criterion's check; do not copy a worker's evidence without running it. Run inspection commands separately; one denied command rejects a whole compound command. List the changed files (committed and uncommitted, against the BRIEF base) and compare them with Scope in; send out-of-scope changes back to the worker to revert, or list them under Out of scope with the reason when they are required. Run the full suite once if your permissions allow; workers run only the tests for packages they changed. Remove temporary files you created outside the worktree. If a worker stops before finishing, resume it to continue. Send unmet items back to the same worker by resuming its session, at most two rounds.\n5. Report: send one consolidated result in the Report format to the requester on the ticket with `final: true`, adding branches and worktrees.",
                n = d.limits.workers_per_ticket,
                steps = d.limits.worker_steps,
                d = d.name,
                r = role.id
            )
        };
        for (kind, template, mode, description, output) in [
            (
                Kind::Lead,
                include_str!("../assets/templates/lead.md"),
                "primary",
                format!("{} (lead)", role.description),
                &mut rendered.leads,
            ),
            (
                Kind::Worker,
                include_str!("../assets/templates/worker.md"),
                "subagent",
                format!(
                    "{} (worker, spawned by the {} lead)",
                    role.description, role.id
                ),
                &mut rendered.workers,
            ),
        ] {
            let model = if kind == Kind::Lead {
                primary
            } else {
                subagent
            };
            let writer = if role.access == Access::Writer && kind == Kind::Worker {
                format!(
                    "If your role writes code or files: use the ticket id and slug from the request. Determine `<root>` by running `git rev-parse --show-toplevel` in the target repo. Whoever writes creates the worktree. If `<root>/.opencode/crewii/worktrees/{}-<ticket>` already exists, reuse it and its branch instead of creating a new one. Otherwise, create a worktree at `<root>/.opencode/crewii/worktrees/{}-<ticket>` on a new branch `crewii/{}-<ticket>-<slug>` from the BRIEF base. `<ticket>` is the ticket you were given (the first `ticket T-n` in your request), not a parent ticket. Commit there if commits are allowed; if a commit is denied, leave the changes uncommitted and report the diff. Report the branch, worktree path, and commits. Never edit the primary checkout. Never push or merge.\n\n",
                    d.name, d.name, d.name
                )
            } else {
                String::new()
            };
            let vars = [
                ("role", role.id.as_str()),
                ("division", d.name.as_str()),
                ("description", role.description.as_str()),
                ("boundary", role.boundary.as_str()),
                ("teammates", teammates.as_str()),
                ("workers", workers.as_str()),
                ("brief", include_str!("../assets/templates/brief.md")),
                ("writer", writer.as_str()),
            ];
            let rules = agent_rules(
                &d.name,
                &role.id,
                role.access,
                kind,
                &role.permissions,
                &d.floor.remove,
            );
            let prompt = template_prompt(template)?;
            let mut text = format!(
                "{}{}{}",
                frontmatter(&description, mode, model, &rules),
                fill(&prompt, &vars)?,
                body.trim_end()
            );
            text.truncate(text.trim_end().len());
            text.push_str("\n\nPersona adapted from agency-agents (MIT, Copyright (c) 2025 AgentLand Contributors); see LICENSE.agency-agents.\n");
            output.push((role.id.clone(), text));
        }
    }
    Ok(rendered)
}

pub fn frontmatter(description: &str, mode: &str, model: Option<&str>, rules: &[Rule]) -> String {
    fn quote(value: &str) -> String {
        use std::fmt::Write;
        let mut result = String::new();
        for c in serde_json::to_string(value)
            .expect("strings always serialize")
            .chars()
        {
            if ('\u{7f}'..='\u{9f}').contains(&c) {
                write!(result, "\\u{:04x}", c as u32).expect("writing to string succeeds");
            } else {
                result.push(c);
            }
        }
        result
    }
    let mut text = format!(
        "---\ndescription: {}\nmode: {}\n",
        quote(description),
        quote(mode)
    );
    if let Some(model) = model {
        text.push_str(&format!("model: {}\n", quote(model)));
    }
    // ponytail: no `steps:` frontmatter; OpenCode's step-cap summary turn 400s on models without assistant prefill
    // (upstream anomalyco/opencode#56250). Restore a worker cap once that is fixed.
    if rules.is_empty() {
        text.push_str("permissions: []\n");
    } else {
        text.push_str("permissions:\n");
        for rule in rules {
            let effect = match rule.effect {
                Effect::Allow => "allow",
                Effect::Deny => "deny",
            };
            text.push_str(&format!(
                "  - action: {}\n    resource: {}\n    effect: {}\n",
                quote(&rule.action),
                quote(&rule.resource),
                quote(effect)
            ));
        }
    }
    text.push_str("---\n");
    text
}

pub fn fill(template: &str, vars: &[(&str, &str)]) -> Result<String> {
    let mut text = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let Some(end) = rest[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + end;
        let key = &rest[start + 2..end];
        let value = vars
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
            .ok_or_else(|| format!("Unfilled placeholder {{{{{key}}}}}; provide its value."))?;
        text.push_str(&rest[..start]);
        text.push_str(value);
        rest = &rest[end + 2..];
    }
    text.push_str(rest);
    Ok(text)
}

fn template_prompt(template: &str) -> Result<String> {
    template
        .replace("\r\n", "\n")
        .strip_suffix("{{persona}}\n")
        .map(str::to_owned)
        .ok_or_else(|| "Template must end with {{persona}}; repair the template.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        perm::{Effect, rule},
        preset::load,
    };

    #[test]
    fn lead_process_guidance() {
        let r = rendered();
        let ba = &r.leads[0].1;
        assert!(ba.contains("send one slice at a time"));
        assert!(ba.contains("use crew_cancel (it interrupts the holder)"));
        for (_, lead) in &r.leads {
            assert!(lead.contains("cite the path"));
            assert!(lead.contains("arrives while you work"));
        }
    }
    #[test]
    fn worker_step_limits_and_scope_guidance() {
        for (override_json, steps) in [(None, 60), (Some(r#"{"limits":{"workerSteps":17}}"#), 17)] {
            let d = load("engineering", override_json).unwrap();
            let r = render(&d, Some("P"), Some("S"), &|_| Ok("persona".into())).unwrap();
            for (_, lead) in &r.leads {
                assert!(!lead.contains("\nsteps:"));
                assert!(
                    lead.contains("Do only what the BRIEF and its acceptance criteria require")
                );
                assert!(lead.contains("under Notes"));
            }
            for (_, worker) in &r.workers {
                assert!(worker.contains("\nmodel: \"S\"\n"));
                assert!(!worker.contains("\nsteps:"));
                assert!(
                    worker.contains("Do only what the BRIEF and its acceptance criteria require")
                );
                assert!(worker.contains("under Notes"));
                assert!(worker.contains("Run only the tests for the packages you changed"));
                assert!(worker.contains(
                    "action: \"crew_status\"\n    resource: \"*\"\n    effect: \"deny\""
                ));
            }
            assert!(r.leads[1].1.contains(&format!("about {steps} steps")));
            assert!(r.leads[1].1.contains("full suite once"));
        }
    }

    #[test]
    fn all_workers_use_subagent_model() {
        let d = load("engineering", None).unwrap();
        let r = render(&d, Some("P"), Some("S"), &|_| Ok("persona".into())).unwrap();
        for (id, text) in &r.leads {
            assert!(text.contains("\nmodel: \"P\"\n"), "{id}");
        }
        for (id, text) in &r.workers {
            assert!(text.contains("\nmodel: \"S\"\n"), "{id}");
        }
    }

    fn rendered() -> Rendered {
        let d = load("engineering", None).unwrap();
        render(&d, None, None, &|_| Ok("PERSONA-X".into())).unwrap()
    }
    #[test]
    fn brief_requires_checks_and_report_format() {
        let r = rendered();
        for (_, text) in r.leads.iter().chain(&r.workers) {
            for expected in ["each with its check", "## Report format", "Out of scope:"] {
                assert!(text.contains(expected), "{expected}");
            }
        }
    }
    #[test]
    fn lead_verifies_scope_and_runs_checks() {
        let r = rendered();
        let lead = &r.leads[1].1;
        for expected in [
            "compare them with Scope in",
            "do not copy a worker's evidence without running it",
            "You cannot edit files",
            "Only ba opens new tickets",
        ] {
            assert!(lead.contains(expected), "{expected}");
        }
    }
    #[test]
    fn worker_ends_with_report_format() {
        for (_, text) in rendered().workers {
            assert!(text.contains("End with your result in the Report format."));
        }
    }
    #[test]
    fn denied_guidance_for_leads_and_workers() {
        let r = rendered();
        for (_, text) in r.leads.iter().chain(&r.workers) {
            assert!(text.contains(
                "\"Permission denied: shell\" means a crew rule blocked part of the command"
            ));
        }
    }
    #[test]
    fn attribution_follows_every_persona() {
        let r = rendered();
        for (_, text) in r.leads.iter().chain(&r.workers) {
            assert!(text.ends_with("PERSONA-X\n\nPersona adapted from agency-agents (MIT, Copyright (c) 2025 AgentLand Contributors); see LICENSE.agency-agents.\n"));
        }
    }

    #[test]
    fn renders_all_roles() {
        let d = load("engineering", None).unwrap();
        let r = rendered();
        let ids: Vec<_> = d.roles.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(
            r.leads.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
            ids
        );
        assert_eq!(
            r.workers.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
            ids
        );
        assert_eq!(ids.len(), 7);
        assert!(
            r.leads[0]
                .1
                .contains("You do not start workers; send BRIEFs to role leads.")
        );
        assert!(r.leads[1].1.contains("at most 5 per ticket"));
        assert!(
            r.leads[1]
                .1
                .contains("agent engineering/worker/architect, background false) and wait for it")
        );
        assert!(
            r.leads[1]
                .1
                .contains(&format!("- ba: {}", d.roles[0].description))
        );
        assert!(
            !r.leads[1]
                .1
                .contains(&format!("- architect: {}", d.roles[1].description))
        );
        assert!(
            r.leads[1]
                .1
                .contains(include_str!("../assets/templates/brief.md").trim())
        );
    }

    #[test]
    fn lead_frontmatter() {
        let r = rendered();
        assert!(r.leads[0].1.starts_with("---\n"));
        assert!(r.leads[0].1.contains("mode: \"primary\""));
        assert!(!r.leads[0].1.contains("\nmodel:"));
    }

    #[test]
    fn unset_models_are_omitted() {
        let d = load("engineering", None).unwrap();
        for (primary, subagent) in [(None, None), (Some("P"), None), (None, Some("S"))] {
            let r = render(&d, primary, subagent, &|_| Ok("persona".into())).unwrap();
            for (_, text) in &r.leads {
                assert_eq!(text.contains("\nmodel:"), primary.is_some());
            }
            for (_, text) in &r.workers {
                assert_eq!(text.contains("\nmodel:"), subagent.is_some());
            }
        }
    }

    #[test]
    fn worker_mode_and_denies() {
        let r = rendered();
        let w = &r.workers[2].1;
        assert!(w.contains("mode: \"subagent\""));
        for action in ["crew_send", "crew_cancel", "question"] {
            assert!(w.contains(&format!(
                "action: \"{action}\"\n    resource: \"*\"\n    effect: \"deny\""
            )));
        }
    }

    #[test]
    fn lead_prompt_describes_dispatch_and_cancellation() {
        let r = rendered();
        let ba = &r.leads.iter().find(|(name, _)| name == "ba").unwrap().1;
        assert!(ba.contains("send every new task with crew_send to a role"));
        assert!(ba.contains("replies stay on their ticket"));
        assert!(ba.contains("`crew_cancel` cancels a ticket you requested"));
    }

    #[test]
    fn persona_last() {
        let r = rendered();
        for (_, text) in r.leads.iter().chain(&r.workers) {
            assert_eq!(
                text.lines().rfind(|s| !s.trim().is_empty()),
                Some(
                    "Persona adapted from agency-agents (MIT, Copyright (c) 2025 AgentLand Contributors); see LICENSE.agency-agents."
                )
            );
        }
    }

    #[test]
    fn persona_braces_are_preserved() {
        let d = load("engineering", None).unwrap();
        let r = render(&d, None, None, &|_| Ok("persona {{x}}".into())).unwrap();
        assert!(r.leads[0].1.contains("persona {{x}}\n\nPersona adapted"));
        assert!(r.workers[0].1.contains("persona {{x}}\n\nPersona adapted"));
    }

    #[test]
    fn no_ask_anywhere() {
        let r = rendered();
        for (_, text) in r.leads.iter().chain(&r.workers) {
            assert!(!text.lines().any(|line| line.trim() == "effect: \"ask\""));
        }
    }

    #[test]
    fn quoting() {
        let f = frontmatter(
            "say \"hi\" \\ bye",
            "primary",
            Some("x\\\"y"),
            &[rule("edit", "*docs/*.md", Effect::Allow)],
        );
        assert!(f.contains("description: \"say \\\"hi\\\" \\\\ bye\""));
        assert!(f.contains("model: \"x\\\\\\\"y\""));
        assert!(f.contains("resource: \"*docs/*.md\""));
    }

    #[test]
    fn fill_leftover_errors() {
        assert!(
            fill("{{a}} {{b}}", &[("a", "1")])
                .unwrap_err()
                .contains("{{b}}")
        );
        assert_eq!(fill("{{a}} {{a}}", &[("a", "1")]).unwrap(), "1 1");
        assert_eq!(fill("unclosed {{a", &[]).unwrap(), "unclosed {{a");
    }

    #[test]
    fn persona_error_propagates() {
        let d = load("engineering", None).unwrap();
        assert!(
            render(&d, None, None, &|_| Err("missing x".into()))
                .unwrap_err()
                .contains("missing x")
        );
    }

    #[test]
    fn substituted_values_are_not_rescanned() {
        assert_eq!(
            fill(
                "{{boundary}} {{teammates}}",
                &[
                    ("boundary", "{{teammates}} {{ .Name }}"),
                    ("teammates", "team")
                ]
            )
            .unwrap(),
            "{{teammates}} {{ .Name }} team"
        );
        let mut d = load("engineering", None).unwrap();
        d.roles[0].boundary = "{{teammates}} {{ .Name }}".into();
        let r = render(&d, None, None, &|_| Ok("persona".into())).unwrap();
        assert!(r.leads[0].1.contains("{{teammates}} {{ .Name }}"));
        assert!(r.workers[0].1.contains("{{teammates}} {{ .Name }}"));
    }

    #[test]
    fn controls_and_empty_permissions() {
        let f = frontmatter(
            "line\n\t\u{0001}\u{007f}\u{0085}\u{009f}",
            "primary",
            None,
            &[],
        );
        assert!(
            f.contains("description: \"line\\n\\t\\u0001\\u007f\\u0085\\u009f\""),
            "{f}"
        );
        assert!(f.contains("permissions: []\n"));
    }

    #[test]
    fn persona_trailing_newlines_are_trimmed() {
        let d = load("engineering", None).unwrap();
        let r = render(&d, None, None, &|_| Ok("persona\n\n".into())).unwrap();
        for (_, text) in r.leads.iter().chain(&r.workers) {
            assert!(text.contains("persona\n\nPersona adapted"));
        }
        let r = render(&d, None, None, &|_| Ok("\n".into())).unwrap();
        assert!(r.leads[0].1.contains("## Persona\n\nPersona adapted"));
    }

    #[test]
    fn crlf_template_normalized() {
        assert_eq!(
            template_prompt("hello\r\n{{persona}}\r\n").unwrap(),
            "hello\n"
        );
        assert!(template_prompt("no persona").is_err());
    }

    #[test]
    fn worker_guidance_and_ba_worker() {
        let r = rendered();
        assert!(
            r.workers[0]
                .1
                .contains(".opencode/crewii/tmp/engineering-<ticket>")
        );
        assert!(r.workers[0].1.contains("default build and package caches"));
        assert!(
            r.workers[0]
                .1
                .contains("stop containers and background processes")
        );
        let sentence = "In every worker prompt, include the ticket id, a short kebab-case slug, the complete BRIEF, and that worker's sub-task with its own acceptance criteria.";
        let ba = r.leads[0]
            .1
            .split("## Workers\n\n")
            .nth(1)
            .unwrap()
            .split("\n\n")
            .next()
            .unwrap();
        let other = r.leads[1]
            .1
            .split("## Workers\n\n")
            .nth(1)
            .unwrap()
            .split("\n\n")
            .next()
            .unwrap();
        assert_eq!(ba, "You do not start workers; send BRIEFs to role leads.");
        assert!(other.contains(sentence));
        for step in [
            "1. Analyse",
            "2. Plan",
            "3. Dispatch",
            "4. Verify",
            "5. Report",
        ] {
            assert!(other.contains(step), "{step}");
        }
        assert!(other.contains("You cannot edit files or change Git state; workers make every change, including git add and commit"));
        assert!(other.contains("Remove temporary files you created outside the worktree"));
        assert!(!other.contains("background true"));
        assert!(other.contains("crewii/engineering-<ticket>-<slug>"));
        assert!(other.contains("never check out or switch branches in the primary checkout"));
        assert_eq!(r.workers[0].0, "ba");
        assert!(
            r.workers[0]
                .1
                .contains("# ba worker of the engineering crew")
        );
    }

    #[test]
    fn writer_instructions_only_for_writers() {
        let r = rendered();
        for (id, text) in r.leads.iter().chain(&r.workers) {
            let writer = ["backend", "frontend", "qa"].contains(&id.as_str())
                && r.workers.iter().any(|(_, worker)| worker == text);
            assert_eq!(text.contains("worktrees/engineering-"), writer, "{id}");
            let before_brief = text.split("## BRIEF: Definition of Ready").next().unwrap();
            assert!(
                !before_brief.contains("\n\n\n"),
                "extra blank line for {id}"
            );
            if writer {
                assert!(text.contains("Whoever writes creates the worktree."));
                assert!(text.contains(
                    "already exists, reuse it and its branch instead of creating a new one."
                ));
                assert!(text.contains("not a parent ticket"));
                assert!(text.contains("Commit there if commits are allowed; if a commit is denied, leave the changes uncommitted and report the diff."));
            }
        }
    }

    #[test]
    fn lead_messaging_distinguishes_delegation_and_reply() {
        let r = rendered();
        let lead = &r.leads[0].1;
        assert!(lead.contains("this opens a linked sub-ticket and you get the result back on it"));
        assert!(lead.contains("A request tagged `[from user ...]` must end with `crew_send` to \"user\" on that ticket with `final: true`."));
        assert!(
            lead.contains("A message typed directly in your tab has no ticket; answer it in chat.")
        );
    }
}
