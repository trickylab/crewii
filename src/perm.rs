//! Permission rules, access profiles, floor, and rule evaluation.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Allow,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub action: String,
    pub resource: String,
    pub effect: Effect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    Coordinator,
    DocsWriter,
    ReadOnly,
    Writer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Lead,
    Worker,
}

pub fn rule(action: &str, resource: &str, effect: Effect) -> Rule {
    Rule {
        action: action.into(),
        resource: resource.into(),
        effect,
    }
}

pub fn profile(access: Access) -> Vec<Rule> {
    let mut rules = Vec::new();
    if access == Access::Writer {
        return rules;
    }
    rules.push(rule("edit", "*", Effect::Deny));
    if matches!(access, Access::Coordinator | Access::DocsWriter) {
        rules.push(rule("edit", "*docs/*.md", Effect::Allow));
    }
    for command in [
        "add",
        "reset",
        "checkout",
        "switch",
        "restore",
        "rebase",
        "cherry-pick",
        "stash",
        "clean",
        "rm",
        "mv",
        "tag",
        "branch",
        "pull",
        "clone",
        "config",
        "worktree",
        "apply",
        "am",
        "revert",
        "commit",
        "push",
        "merge",
        "remote",
        "submodule",
        "gc",
        "prune",
        "update-ref",
        "notes",
    ] {
        let suffix = if command == "merge" { " *" } else { "*" };
        rules.push(rule(
            "shell",
            &format!("git {command}{suffix}"),
            Effect::Deny,
        ));
        rules.push(rule(
            "shell",
            &format!("git -C * {command}{suffix}"),
            Effect::Deny,
        ));
    }
    // Read-only forms of denied subcommands; a denied part rejects the whole compound command.
    for read in [
        "branch",
        "branch --show-current",
        "branch -a",
        "branch -r",
        "branch -v",
        "branch -vv",
        "branch --list*",
        "branch --contains*",
        "branch -r --contains*",
        "branch -a --contains*",
        "branch --merged*",
        "branch --no-merged*",
        "branch -r --merged*",
        "branch -a --merged*",
        "worktree list*",
        "stash list*",
        "stash show*",
        "tag -l*",
        "tag --list*",
        "remote -v",
        "remote show*",
        "config --get*",
        "config --list",
    ] {
        rules.push(rule("shell", &format!("git {read}"), Effect::Allow));
        rules.push(rule("shell", &format!("git -C * {read}"), Effect::Allow));
    }
    rules
}

pub fn floor(remove: &[String]) -> Vec<Rule> {
    let mut rules = Vec::new();
    rules.push(rule("read", "*.env", Effect::Deny));
    rules.push(rule("read", "*.env.*", Effect::Deny));
    for pattern in ["*.example", "*.sample", "*.template"] {
        rules.push(rule("read", pattern, Effect::Allow));
    }
    for prefix in [" ", "/", "'", "\""] {
        for suffix in ["", " *", ".*", "'*", "\"*", ";*", "|*", ")*"] {
            rules.push(rule(
                "shell",
                &format!("*{prefix}.env{suffix}"),
                Effect::Deny,
            ));
        }
    }
    // ponytail: example allows also win when a command names both .env and an example;
    // these are guardrails, not a sandbox.
    for pattern in [
        "*.env.example*",
        "*.env.*.example*",
        "*.env.sample*",
        "*.env.template*",
    ] {
        rules.push(rule("shell", pattern, Effect::Allow));
    }
    for pattern in [
        "git push*",
        "git merge",
        "git merge *",
        "git -* push*",
        "git -* merge",
        "git -* merge *",
        "sudo*",
        "gh pr merge*",
        "gh pr create*",
        "gh pr comment*",
        "gh pr review*",
        "gh pr close*",
        "gh pr edit*",
        "gh issue create*",
        "gh issue comment*",
        "gh issue close*",
        "gh issue edit*",
        "gh release*",
        "gh repo*",
        "gh api*",
        "gh workflow run*",
        "kubectl*",
    ] {
        rules.push(rule("shell", pattern, Effect::Deny));
    }
    for pattern in ["*/.ssh/*", "*/.aws/*", "*/.netrc*", "*/.config/gh/*"] {
        rules.push(rule("read", pattern, Effect::Deny));
        rules.push(rule("shell", &format!("*{pattern}*"), Effect::Deny));
    }
    // Read paths are absolute, so `~/` forms only matter for shell (OpenCode expands `~/` in read rules).
    for pattern in ["~/.ssh*", "~/.aws*", "~/.netrc*", "~/.config/gh*"] {
        rules.push(rule("shell", &format!("*{pattern}*"), Effect::Deny));
    }
    rules.retain(|r| !remove.contains(&r.resource));
    rules
}

pub fn agent_rules(
    division: &str,
    role: &str,
    access: Access,
    kind: Kind,
    extra: &[Rule],
    floor_remove: &[String],
) -> Vec<Rule> {
    let mut rules = profile(if kind == Kind::Lead {
        Access::ReadOnly
    } else {
        access
    });
    rules.push(rule("subagent", "*", Effect::Deny));
    match kind {
        Kind::Lead if role != "ba" => rules.push(rule(
            "subagent",
            &format!("{division}/worker/{role}"),
            Effect::Allow,
        )),
        Kind::Lead => {}
        Kind::Worker => {
            rules.push(rule("crew_send", "*", Effect::Deny));
            rules.push(rule("crew_status", "*", Effect::Deny));
            rules.push(rule("crew_cancel", "*", Effect::Deny));
            rules.push(rule("question", "*", Effect::Deny));
        }
    }
    rules.extend_from_slice(extra);
    // ponytail: leads never edit; role overrides cannot re-allow it. Upgrade: split role permissions into lead/worker if needed.
    if kind == Kind::Lead {
        rules.push(rule("edit", "*", Effect::Deny));
    }
    rules.extend(floor(floor_remove));
    rules
}

pub fn matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut p, mut v, mut star, mut retry) = (0, 0, None, 0);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == value[v]) && pattern[p] != '*' {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            p += 1;
            retry = v;
        } else if let Some(s) = star {
            retry += 1;
            v = retry;
            p = s + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

pub fn effective<'a>(
    rules: &[(&'a str, &'a str, &'a str)],
    action: &str,
    resource: &str,
) -> &'a str {
    rules
        .iter()
        .rev()
        .find(|(a, r, _)| {
            matches(a, action)
                && (matches(r, resource)
                    || (action == "shell"
                        && r.strip_suffix(" *")
                            .is_some_and(|bare| matches(bare, resource))))
        })
        .map_or("ask", |(_, _, effect)| *effect)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evaluate(rules: &[Rule], action: &str, resource: &str) -> String {
        let mut tuples = vec![
            ("*", "*", "allow"),
            ("read", "*.env", "ask"),
            ("read", "*.env.*", "ask"),
            ("read", "*.env.example", "allow"),
        ];
        tuples.extend(rules.iter().map(|r| {
            (
                r.action.as_str(),
                r.resource.as_str(),
                match r.effect {
                    Effect::Allow => "allow",
                    Effect::Deny => "deny",
                },
            )
        }));
        effective(&tuples, action, resource).to_string()
    }
    fn agent(access: Access, kind: Kind) -> Vec<Rule> {
        agent_rules("engineering", "backend", access, kind, &[], &[])
    }
    #[test]
    fn lead_edit_deny_beats_role_override() {
        let extra = [rule(
            "edit",
            "*/.opencode/crewii/worktrees/*",
            Effect::Allow,
        )];
        for (kind, expected) in [(Kind::Lead, "deny"), (Kind::Worker, "allow")] {
            let rules = agent_rules("engineering", "backend", Access::Writer, kind, &extra, &[]);
            assert_eq!(
                evaluate(&rules, "edit", "/r/.opencode/crewii/worktrees/x/a.go"),
                expected
            );
        }
    }
    #[test]
    fn writer_lead_has_read_only_git() {
        let lead = agent(Access::Writer, Kind::Lead);
        for command in ["git commit -m x", "git add .", "git checkout main"] {
            assert_eq!(evaluate(&lead, "shell", command), "deny");
        }
        for command in ["git diff", "git status", "git log"] {
            assert_ne!(evaluate(&lead, "shell", command), "deny");
        }
        assert_ne!(
            evaluate(
                &agent(Access::Writer, Kind::Worker),
                "shell",
                "git commit -m x"
            ),
            "deny"
        );
    }
    #[test]
    fn leads_cannot_edit_docs() {
        for access in [Access::Coordinator, Access::DocsWriter] {
            assert_eq!(
                evaluate(&agent(access, Kind::Lead), "edit", "/r/docs/a.md"),
                "deny"
            );
        }
        assert_eq!(
            evaluate(
                &agent(Access::DocsWriter, Kind::Worker),
                "edit",
                "/r/docs/a.md"
            ),
            "allow"
        );
    }
    #[test]
    fn inspection_commands_and_env_examples_are_allowed() {
        for kind in [Kind::Lead, Kind::Worker] {
            let rules = agent(Access::ReadOnly, kind);
            for command in [
                "sed -n 1,5p a.go",
                "python3 -c 1",
                "go vet ./...",
                "docker ps",
                "git -c core.pager=cat diff",
                "cat a > /tmp/x",
                "git fetch origin",
                "git diff -- .env.example",
                "cat .env.stg.example",
                "rg process.env src",
                "rg import.meta.env src",
            ] {
                assert_eq!(
                    evaluate(&rules, "shell", command),
                    "allow",
                    "{kind:?}: {command}"
                );
            }
            for command in [
                "git commit -m x",
                "git checkout main",
                "git push",
                "git -C r reset --hard",
                "cat .env",
                "cat ./.env.local",
                "source .env;x",
                "cat '.env'",
            ] {
                assert_eq!(
                    evaluate(&rules, "shell", command),
                    "deny",
                    "{kind:?}: {command}"
                );
            }
            assert_eq!(evaluate(&rules, "read", "/r/.env.prod.example"), "allow");
            assert_eq!(evaluate(&rules, "read", "/r/.env.prod"), "deny");
            for path in ["/r/.env.sample", "/r/.env.template", "/r/.env.prod.sample"] {
                assert_eq!(evaluate(&rules, "read", path), "allow", "{path}");
            }
            for command in [
                "cat .env.sample",
                "cat .env.template",
                "cat .env .env.example",
            ] {
                assert_eq!(evaluate(&rules, "shell", command), "allow", "{command}");
            }
            assert_eq!(evaluate(&rules, "edit", "docs/a.md"), "deny");
        }
        assert_eq!(
            evaluate(
                &agent(Access::Coordinator, Kind::Lead),
                "shell",
                "docker ps"
            ),
            "allow"
        );
        assert_eq!(
            evaluate(
                &agent(Access::DocsWriter, Kind::Worker),
                "edit",
                "docs/a.md"
            ),
            "allow"
        );
    }
    #[test]
    fn git_push_and_merge_options_are_denied() {
        let rules = agent(Access::Writer, Kind::Lead);
        for command in [
            "git -P push origin x",
            "git -p push",
            "git -P -C x push",
            "git -P merge main",
            "git -P -C x merge main",
            "git --no-pager push",
            "git push",
            "git -C/tmp push origin main",
            "git -C/tmp merge main",
            "git -c a=b push",
        ] {
            assert_eq!(evaluate(&rules, "shell", command), "deny", "{command}");
        }
        assert_eq!(evaluate(&rules, "shell", "git status"), "allow");
    }
    #[test]
    fn git_merge_base_is_allowed_for_read_only() {
        let rules = agent(Access::ReadOnly, Kind::Lead);
        for command in ["git merge-base a b", "git -C x merge-base a b"] {
            assert_eq!(evaluate(&rules, "shell", command), "allow", "{command}");
        }
    }
    #[test]
    fn env_files_are_denied_in_shell() {
        for access in [Access::Writer, Access::ReadOnly] {
            let rules = agent(access, Kind::Lead);
            for command in [
                "cat .env",
                "grep -r KEY .env",
                "grep KEY .env.local",
                "cat .env extra",
                "cat .env.local extra",
                "cat '.env'",
                "cat \".env\"",
                "cat '.env.local'",
                "cat \".env.local\"",
            ] {
                assert_eq!(
                    evaluate(&rules, "shell", command),
                    "deny",
                    "{access:?} {command}"
                );
            }
            assert_eq!(
                evaluate(&rules, "shell", "git push origin .env.example"),
                "deny"
            );
            assert_eq!(evaluate(&rules, "read", "/r/.env.example"), "allow");
            for command in ["cat src/env.rs", "ls"] {
                assert_eq!(
                    evaluate(&rules, "shell", command),
                    "allow",
                    "{access:?} {command}"
                );
            }
        }
    }
    #[test]
    fn home_secrets_are_denied_for_read_and_shell() {
        for access in [Access::Writer, Access::ReadOnly] {
            let rules = agent(access, Kind::Lead);
            for command in [
                "cat ~/.ssh/id_ed25519",
                "cat /home/u/.aws/credentials",
                "cat ~/.config/gh/hosts.yml",
                "cat ~/.aws/credentials",
                "cat ~/.netrc",
                "cat /home/u/.netrc",
                "cat /home/u/.config/gh/hosts.yml",
            ] {
                assert_eq!(
                    evaluate(&rules, "shell", command),
                    "deny",
                    "{access:?} {command}"
                );
            }
            for path in [
                "/home/u/.ssh/id_rsa",
                "/home/u/.aws/credentials",
                "/home/u/.netrc",
                "/home/u/.config/gh/hosts.yml",
            ] {
                assert_eq!(evaluate(&rules, "read", path), "deny", "{access:?} {path}");
            }
            assert!(
                !rules
                    .iter()
                    .any(|r| r.action == "read" && r.resource.starts_with('~'))
            );
            for command in ["ls", "cat src/ssh.rs"] {
                assert_eq!(
                    evaluate(&rules, "shell", command),
                    "allow",
                    "{access:?} {command}"
                );
            }
        }
    }
    #[test]
    fn profiles_preserve_user_global_permissions() {
        for access in [
            Access::Coordinator,
            Access::DocsWriter,
            Access::ReadOnly,
            Access::Writer,
        ] {
            let rules = profile(access);
            assert!(!rules.iter().any(|r| r.action == "*" && r.resource == "*"));
            let mut tuples = vec![
                ("*", "*", "allow"),
                ("webfetch", "*", "deny"),
                ("my_mcp_write", "*", "deny"),
            ];
            tuples.extend(rules.iter().map(|r| {
                (
                    r.action.as_str(),
                    r.resource.as_str(),
                    match r.effect {
                        Effect::Allow => "allow",
                        Effect::Deny => "deny",
                    },
                )
            }));
            assert_eq!(effective(&tuples, "webfetch", "url"), "deny");
            assert_eq!(effective(&tuples, "my_mcp_write", "x"), "deny");
        }
        assert!(profile(Access::Writer).is_empty());
    }
    #[test]
    fn server_base_rules_are_prepended() {
        assert_eq!(evaluate(&[], "shell", "ls"), "allow");
        assert_eq!(evaluate(&[], "read", "a/.env"), "ask");
        assert_eq!(evaluate(&[], "read", "a/.env.local"), "ask");
        assert_eq!(evaluate(&[], "read", "a/.env.example"), "allow");
    }
    #[test]
    fn review_shell_escapes() {
        let rules = agent(Access::ReadOnly, Kind::Lead);
        for sub in [
            "add",
            "reset",
            "checkout",
            "switch",
            "restore",
            "rebase",
            "cherry-pick",
            "stash",
            "clean",
            "rm",
            "mv",
            "tag",
            "branch",
            "pull",
            "clone",
            "config",
            "worktree",
            "apply",
            "am",
            "revert",
            "commit",
            "push",
            "merge",
            "remote",
            "submodule",
            "gc",
            "prune",
            "update-ref",
            "notes",
        ] {
            for command in [
                format!("git {sub} status"),
                format!("git -C a {sub} status"),
            ] {
                assert_eq!(evaluate(&rules, "shell", &command), "deny", "{command}");
            }
        }
        for sub in [
            "merge-base",
            "rev-list",
            "diff-tree",
            "show-ref",
            "for-each-ref",
            "name-rev",
        ] {
            for command in [format!("git {sub} HEAD"), format!("git -C a {sub} HEAD")] {
                assert_eq!(evaluate(&rules, "shell", &command), "allow", "{command}");
            }
        }
    }
    #[test]
    fn review_writer_floor_global_options() {
        let rules = agent(Access::Writer, Kind::Lead);
        for command in [
            "git --no-pager push",
            "git -c x=y push",
            "git --no-pager merge",
            "git -c x=y merge",
        ] {
            assert_eq!(evaluate(&rules, "shell", command), "deny", "{command}");
        }
    }
    #[test]
    fn review_rule_json_validation() {
        assert!(
            serde_json::from_str::<Rule>(
                r#"{"action":"shell","resource":"*","effect":"allow","extra":true}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Rule>(r#"{"action":"shell","resource":"*","effect":"ask"}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<Rule>(r#"{"action":"shell","resource":"*","effect":"allow"}"#)
                .is_ok()
        );
    }
    #[test]
    fn glob() {
        for (pattern, value, expected) in [
            ("*.env", "a/.env", true),
            ("a.b", "axb", false),
            ("a?c", "abc", true),
            ("*a*b", "xxa/yb", true),
            ("*a*b", "xxa/yc", false),
            ("?", "é", true),
            ("", "", true),
            ("a*", "a", true),
            ("a", "ab", false),
            ("*", "", true),
        ] {
            assert_eq!(matches(pattern, value), expected, "{pattern:?} {value:?}");
        }
    }
    #[test]
    fn bare_command() {
        assert_eq!(
            effective(&[("shell", "git status *", "allow")], "shell", "git status"),
            "allow"
        );
        assert_eq!(
            effective(&[("read", "foo *", "allow")], "read", "foo"),
            "ask"
        );
    }
    #[test]
    fn last_match_wins_and_default_ask() {
        let rules = [
            ("*", "*", "allow"),
            ("shell", "git *", "deny"),
            ("shell", "git status *", "allow"),
        ];
        assert_eq!(effective(&rules, "shell", "git status"), "allow");
        assert_eq!(effective(&rules, "shell", "git push"), "deny");
        assert_eq!(effective(&[], "read", "x"), "ask");
    }
    #[test]
    fn read_only_matrix() {
        let rules = agent(Access::ReadOnly, Kind::Lead);
        for command in [
            "grep -rn x . 2>/dev/null",
            "git -C a b show HEAD:x",
            "find . -name '*.go'",
            "gh pr view 1",
            "ls",
            "git status",
            "git -C a status",
            "git branch",
            "git branch --show-current",
            "git branch -r --contains HEAD",
            "git -C a branch --merged main",
            "git -C a branch -vv",
            "git worktree list --porcelain",
            "git stash list",
            "git config --get user.name",
        ] {
            assert_eq!(evaluate(&rules, "shell", command), "allow", "{command}");
        }
        for command in [
            "git commit -m x",
            "git branch -D main",
            "git branch new",
            "git worktree add x",
            "git stash",
            "git stash pop",
            "git config user.name x",
        ] {
            assert_eq!(evaluate(&rules, "shell", command), "deny", "{command}");
        }
        assert_eq!(evaluate(&rules, "edit", "docs/a.md"), "deny");
    }
    #[test]
    fn coordinator_matrix() {
        let rules = agent_rules(
            "engineering",
            "ba",
            Access::Coordinator,
            Kind::Worker,
            &[],
            &[],
        );
        for (action, resource, expected) in [
            ("shell", "ls", "allow"),
            ("edit", "/r/docs/a.md", "allow"),
            ("edit", "/r/src/a.rs", "deny"),
            ("subagent", "engineering/worker/ba", "deny"),
            ("crew_send", "x", "deny"),
        ] {
            assert_eq!(
                evaluate(&rules, action, resource),
                expected,
                "{action} {resource}"
            );
        }
    }
    #[test]
    fn docs_writer_matrix() {
        let rules = agent(Access::DocsWriter, Kind::Worker);
        for (action, resource, expected) in [
            ("edit", "/r/docs/a.md", "allow"),
            ("edit", "/r/src/a.rs", "deny"),
            ("shell", "cat x", "allow"),
            ("shell", "go test", "allow"),
        ] {
            assert_eq!(evaluate(&rules, action, resource), expected);
        }
    }
    #[test]
    fn writer_matrix() {
        let rules = agent(Access::Writer, Kind::Worker);
        assert_eq!(evaluate(&rules, "shell", "go test ./..."), "allow");
        assert_eq!(evaluate(&rules, "edit", "/r/src/a.rs"), "allow");
    }
    #[test]
    fn floor_for_every_access() {
        for access in [
            Access::Coordinator,
            Access::DocsWriter,
            Access::ReadOnly,
            Access::Writer,
        ] {
            let rules = agent(access, Kind::Lead);
            for command in [
                "git push origin x",
                "git -C r push",
                "git merge main",
                "git -C a b merge main",
                "sudo ls",
                "gh pr merge 1",
                "gh pr create",
                "gh pr comment 1",
                "gh pr review 1",
                "gh pr close 1",
                "gh pr edit 1",
                "gh issue create",
                "gh issue comment 1",
                "gh issue close 1",
                "gh issue edit 1",
                "gh release",
                "gh repo",
                "gh api x",
                "gh workflow run x",
                "kubectl get pods",
            ] {
                assert_eq!(
                    evaluate(&rules, "shell", command),
                    "deny",
                    "{access:?} {command}"
                );
            }
            for file in [
                ".env",
                "a/.env.local",
                "a/secrets.env",
                "a/secrets.env.production",
            ] {
                assert_eq!(evaluate(&rules, "read", file), "deny", "{file}");
            }
            assert_eq!(evaluate(&rules, "read", "a/.env.example"), "allow");
        }
    }
    #[test]
    fn lead_and_worker_rules() {
        let lead = agent(Access::Writer, Kind::Lead);
        assert_eq!(evaluate(&lead, "crew_status", "x"), "allow");
        for (target, expected) in [
            ("engineering/worker/backend", "allow"),
            ("engineering/worker/other", "deny"),
            ("general", "deny"),
        ] {
            assert_eq!(evaluate(&lead, "subagent", target), expected);
        }
        let worker = agent(Access::Writer, Kind::Worker);
        for action in [
            "subagent",
            "crew_send",
            "crew_status",
            "crew_cancel",
            "question",
        ] {
            assert_eq!(evaluate(&worker, action, "x"), "deny");
        }
    }
    #[test]
    fn floor_remove_is_explicit() {
        let remove = ["gh api*".to_string()];
        assert!(!floor(&remove).iter().any(|r| r.resource == "gh api*"));
        let rules = agent_rules(
            "engineering",
            "backend",
            Access::Writer,
            Kind::Lead,
            &[],
            &remove,
        );
        assert_eq!(evaluate(&rules, "shell", "gh api x"), "allow");
    }
    #[test]
    fn extra_rules_cannot_beat_floor() {
        let extra = [rule("shell", "git push*", Effect::Allow)];
        let rules = agent_rules(
            "engineering",
            "backend",
            Access::Writer,
            Kind::Lead,
            &extra,
            &[],
        );
        assert_eq!(evaluate(&rules, "shell", "git push x"), "deny");
    }
}
