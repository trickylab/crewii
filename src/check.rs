//! Isolation and permission assertions against a running OpenCode.

use crate::{Result, opencode::Api, paths::Layout, preset::Division};

pub fn check(api: &dyn Api, layout: &Layout, d: &Division) -> Result<Vec<String>> {
    use crate::perm::{self, Effect, Kind};
    use std::collections::BTreeSet;

    let root = api.get(&layout.root, "/api/agent")?;
    let root = root
        .as_array()
        .ok_or("Invalid root agent response; check the OpenCode version")?;
    let division = api.get(&layout.division_dir(), "/api/agent")?;
    let agents = division
        .as_array()
        .ok_or("Invalid division agent response; check the OpenCode version")?;
    let mut failures = Vec::new();
    let prefix = format!("{}/", d.name);
    for agent in root {
        let id = agent
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Invalid root agent id; check the OpenCode version")?;
        if id.starts_with(&prefix) {
            failures.push(format!("isolation: {id} is visible from the project root; remove or rename the agent file that defines it outside .opencode/crewii"));
        }
    }
    for role in &d.roles {
        for kind in [Kind::Lead, Kind::Worker] {
            let id = format!(
                "{}{}{}",
                prefix,
                if kind == Kind::Worker { "worker/" } else { "" },
                role.id
            );
            let Some(agent) = agents
                .iter()
                .find(|a| a.get("id").and_then(|v| v.as_str()) == Some(&id))
            else {
                failures.push(format!(
                    "missing agent {id}; run `crewii {}` to rebuild",
                    d.name
                ));
                continue;
            };
            let start = failures.len();
            let (kind_name, mode) = if kind == Kind::Worker {
                ("worker", "subagent")
            } else {
                ("lead", "primary")
            };
            if agent.get("mode").and_then(|v| v.as_str()) != Some(mode) {
                failures.push(format!(
                    "{id}: {kind_name} mode must be {mode}; run `crewii {}` to rebuild",
                    d.name
                ));
            }
            let rules = agent
                .get("permissions")
                .and_then(|v| v.as_array())
                .ok_or_else(|| format!("{id}: invalid permissions; check the OpenCode version"))?;
            let tuples = rules
                .iter()
                .map(|r| {
                    let invalid =
                        || format!("{id}: invalid permission rule; check the OpenCode version");
                    let action = r
                        .get("action")
                        .and_then(|v| v.as_str())
                        .ok_or_else(invalid)?;
                    let resource = r
                        .get("resource")
                        .and_then(|v| v.as_str())
                        .ok_or_else(invalid)?;
                    let effect = r
                        .get("effect")
                        .and_then(|v| v.as_str())
                        .filter(|v| matches!(*v, "allow" | "deny" | "ask"))
                        .ok_or_else(invalid)?;
                    Ok((action, resource, effect))
                })
                .collect::<Result<Vec<_>>>()?;
            let expected = perm::agent_rules(
                &d.name,
                &role.id,
                role.access,
                kind,
                &role.permissions,
                &d.floor.remove,
            );
            let expected_tuples: Vec<_> = expected
                .iter()
                .map(|r| {
                    (
                        r.action.as_str(),
                        r.resource.as_str(),
                        match r.effect {
                            Effect::Allow => "allow",
                            Effect::Deny => "deny",
                        },
                    )
                })
                .collect();
            let mut probes: BTreeSet<(String, String)> = [
                ("edit", "/x"),
                ("read", "/x"),
                ("shell", "ls"),
                ("webfetch", "https://x"),
                ("subagent", "general"),
                ("question", "x"),
                ("crew_send", "x"),
                ("crew_status", "x"),
                ("crew_cancel", "x"),
                ("edit", "/r/docs/a.md"),
                ("edit", "/r/src/a.rs"),
                ("edit", "/r/.opencode/crewii/worktrees/x/a.rs"),
                ("edit", "docs/a.md"),
                ("shell", "cat x"),
                ("shell", "go test"),
                ("shell", "go test ./..."),
                ("shell", "git push origin x"),
                ("shell", "git -C r push"),
                ("shell", "git merge main"),
                ("shell", "git -C a b merge main"),
                ("shell", "git --no-pager push"),
                ("shell", "git -c x=y push"),
                ("shell", "git --no-pager merge"),
                ("shell", "git -c x=y merge"),
                ("shell", "sudo ls"),
                ("shell", "gh pr merge 1"),
                ("shell", "gh pr create"),
                ("shell", "gh pr comment 1"),
                ("shell", "gh pr review 1"),
                ("shell", "gh pr close 1"),
                ("shell", "gh pr edit 1"),
                ("shell", "gh issue create"),
                ("shell", "gh issue comment 1"),
                ("shell", "gh issue close 1"),
                ("shell", "gh issue edit 1"),
                ("shell", "gh release"),
                ("shell", "gh repo"),
                ("shell", "gh api x"),
                ("shell", "gh workflow run x"),
                ("shell", "kubectl get pods"),
                ("read", ".env"),
                ("read", "a/.env.local"),
                ("read", "a/secrets.env"),
                ("read", "a/secrets.env.production"),
                ("read", "a/.env.example"),
                ("shell", "grep -rn x . 2>/dev/null"),
                ("shell", "git -C a b show HEAD:x"),
                ("shell", "find . -name '*.go'"),
                ("shell", "gh pr view 1"),
                ("shell", "git status"),
                ("shell", "git -C a status"),
                ("shell", "rm -rf x"),
                ("shell", "sed -i s/a/b/ f"),
                ("shell", "sed -e s/a/b/ -i f"),
                ("shell", "sed --in-place f"),
                ("shell", "sort -o f"),
                ("shell", "sort -r -o f"),
                ("shell", "sort --output=f"),
                ("shell", "echo x > f"),
                ("shell", "grep a x >> f"),
                ("shell", "echo x>>f"),
                ("shell", "echo x &> f"),
                ("shell", "find . -delete"),
                ("shell", "find . -exec rm {} ;"),
                ("shell", "find . -ok cmd ;"),
                ("shell", "find . -fprint f"),
                ("shell", "find . -fls f"),
                ("shell", "git commit -m x"),
                ("shell", "git branch -D main"),
            ]
            .into_iter()
            .map(|(a, r)| (a.to_owned(), r.to_owned()))
            .collect();
            for other in &d.roles {
                probes.insert(("subagent".into(), format!("{}/worker/{}", d.name, other.id)));
                probes.insert(("subagent".into(), format!("{}/{}", d.name, other.id)));
            }
            for (action, resource, _) in tuples.iter().chain(expected_tuples.iter()) {
                let action = action.replace(['*', '?'], "x");
                let resource = resource.replace(['*', '?'], "x");
                probes.insert((action.clone(), resource));
                probes.insert((action, "x".into()));
            }
            for (action, resource) in probes {
                let actual = perm::effective(&tuples, &action, &resource);
                let want = perm::effective(&expected_tuples, &action, &resource);
                if actual == "ask" {
                    failures.push(format!("{id}: {action} {resource} would prompt (ask); background agents cannot answer — add an allow or deny for it in .opencode/crewii/{}.json", d.name));
                } else if actual == "allow" && want == "deny" {
                    failures.push(format!(
                        "{id}: {action} {resource} is {actual}, expected {want}"
                    ));
                }
            }
            let count = failures.len() - start;
            if count > 10 {
                failures.truncate(start + 10);
                failures.push(format!("… {} more for {id}", count - 10));
            }
        }
    }
    Ok(failures)
}

pub fn ok_line(d: &Division) -> String {
    format!("ok: {} leads + {} workers", d.roles.len(), d.roles.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{opencode::Fake, preset, render};
    use serde_json::{Value, json};
    use std::path::Path;

    struct Server {
        root: Value,
        agents: Value,
    }
    impl Api for Server {
        fn get(&self, dir: &Path, path: &str) -> Result<Value> {
            let fake = Fake::new();
            fake.on(
                "get",
                path,
                if dir.ends_with("root") {
                    self.root.clone()
                } else {
                    self.agents.clone()
                },
            );
            fake.get(dir, path)
        }
        fn post(&self, _: &Path, _: &str, _: &Value) -> Result<Value> {
            unreachable!()
        }
        fn delete(&self, _: &Path, _: &str) -> Result<()> {
            unreachable!()
        }
    }
    fn fixture() -> (Division, Layout, Server) {
        let d = preset::load("engineering", None).unwrap();
        let rendered = render::render(&d, None, None, &|_| Ok("persona".into())).unwrap();
        let mut agents = Vec::new();
        for (worker, items) in [(false, rendered.leads), (true, rendered.workers)] {
            for (role, text) in items {
                let mut rules = vec![
                    json!({"action":"*","resource":"*","effect":"allow"}),
                    json!({"action":"read","resource":"*.env","effect":"ask"}),
                    json!({"action":"read","resource":"*.env.*","effect":"ask"}),
                    json!({"action":"read","resource":"*.env.example","effect":"allow"}),
                ];
                let mut action = String::new();
                let mut resource = String::new();
                for line in text.split("---\n").nth(1).unwrap().lines() {
                    let line = line.trim();
                    if let Some(v) = line.strip_prefix("- action: ") {
                        action = serde_json::from_str(v).unwrap();
                    }
                    if let Some(v) = line.strip_prefix("resource: ") {
                        resource = serde_json::from_str(v).unwrap();
                    }
                    if let Some(v) = line.strip_prefix("effect: ") {
                        let effect: String = serde_json::from_str(v).unwrap();
                        rules.push(json!({"action":action,"resource":resource,"effect":effect}));
                    }
                }
                rules.push(json!({"action":"browser","resource":"*","effect":"deny"}));
                rules.push(json!({"action":"skill","resource":"browser","effect":"deny"}));
                agents.push(json!({"id": format!("engineering/{}{role}", if worker { "worker/" } else { "" }), "mode": if worker {"subagent"} else {"primary"}, "permissions": rules}));
            }
        }
        (
            d,
            Layout::new("/root".into(), "engineering"),
            Server {
                root: json!([]),
                agents: json!(agents),
            },
        )
    }
    #[test]
    fn clean_build_passes() {
        let (d, l, s) = fixture();
        assert_eq!(check(&s, &l, &d).unwrap(), Vec::<String>::new());
        assert_eq!(ok_line(&d), "ok: 7 leads + 7 workers");
    }
    #[test]
    fn leak_detected() {
        let (d, l, mut s) = fixture();
        s.root = json!([{"id":"engineering/ba"}]);
        let text = check(&s, &l, &d).unwrap().join(" ");
        assert!(text.contains("isolation"));
        assert!(
            text.contains(
                "remove or rename the agent file that defines it outside .opencode/crewii"
            )
        );
    }
    #[test]
    fn ask_detected() {
        let (d, l, mut s) = fixture();
        s.agents[0]["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"action":"webfetch","resource":"*","effect":"ask"}));
        let text = check(&s, &l, &d).unwrap().join(" ");
        assert!(text.contains("engineering/ba: webfetch") && text.contains("would prompt (ask); background agents cannot answer — add an allow or deny for it in .opencode/crewii/engineering.json"));
    }
    #[test]
    fn base_env_ask_overridden() {
        let (d, l, s) = fixture();
        assert!(!check(&s, &l, &d).unwrap().join(" ").contains("ask"));
    }
    #[test]
    fn floor_gap_detected() {
        let (d, l, mut s) = fixture();
        let agent = s
            .agents
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|a| a["id"] == "engineering/backend")
            .unwrap();
        agent["permissions"]
            .as_array_mut()
            .unwrap()
            .retain(|r| r["resource"] != "git push*");
        assert!(check(&s, &l, &d).unwrap().join(" ").contains("git push"));
    }
    #[test]
    fn missing_worker_detected() {
        let (d, l, mut s) = fixture();
        s.agents
            .as_array_mut()
            .unwrap()
            .retain(|a| a["id"] != "engineering/worker/ba");
        assert!(
            check(&s, &l, &d)
                .unwrap()
                .join(" ")
                .contains("engineering/worker/ba")
        );
        assert!(
            check(&s, &l, &d)
                .unwrap()
                .join(" ")
                .contains("run `crewii engineering` to rebuild")
        );
    }
    #[test]
    fn malformed_responses_fail_closed() {
        let (d, l, mut s) = fixture();
        s.root = json!({});
        assert!(check(&s, &l, &d).unwrap_err().contains("root"));
        s.root = json!([{}]);
        assert!(check(&s, &l, &d).unwrap_err().contains("id"));
        s.root = json!([]);
        s.agents = json!({});
        assert!(check(&s, &l, &d).unwrap_err().contains("division"));
        let (d, l, mut s) = fixture();
        s.agents[0]["permissions"] = json!({});
        assert!(check(&s, &l, &d).unwrap_err().contains("permissions"));
        let (d, l, mut s) = fixture();
        s.agents[0]["permissions"][0] = json!({"action":"*","resource":"*","effect":"unknown"});
        assert!(check(&s, &l, &d).unwrap_err().contains("rule"));
        s.agents[0]["permissions"][0] = json!({});
        assert!(check(&s, &l, &d).unwrap_err().contains("rule"));
        let fake = Fake::new();
        assert!(
            check(&fake, &l, &d)
                .unwrap_err()
                .contains("No fake response")
        );
    }
    #[test]
    fn mode_and_permissions_checked() {
        let (d, l, mut s) = fixture();
        let agents = s.agents.as_array_mut().unwrap();
        let worker = agents
            .iter_mut()
            .find(|a| a["id"] == "engineering/worker/backend")
            .unwrap();
        worker["mode"] = json!("primary");
        worker["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"action":"crew_send","resource":"*","effect":"allow"}));
        worker["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"action":"crew_status","resource":"*","effect":"allow"}));
        worker["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"action":"crew_cancel","resource":"*","effect":"allow"}));
        let text = check(&s, &l, &d).unwrap().join("\n");
        assert!(text.contains("worker mode"));
        assert!(text.contains("crew_send x is allow, expected deny"));
        assert!(text.contains("crew_status x is allow, expected deny"));
        assert!(text.contains("crew_cancel x is allow, expected deny"));
    }
    #[test]
    fn overridden_ask_is_not_reported() {
        let (d, l, mut s) = fixture();
        let rules = s.agents[0]["permissions"].as_array_mut().unwrap();
        rules.push(json!({"action":"webfetch","resource":"*","effect":"ask"}));
        rules.push(json!({"action":"webfetch","resource":"*","effect":"allow"}));
        assert!(check(&s, &l, &d).unwrap().is_empty());
    }

    #[test]
    fn unsafe_server_override_is_rejected() {
        let (d, l, mut s) = fixture();
        let agent = s
            .agents
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|a| a["id"] == "engineering/backend")
            .unwrap();
        agent["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"action":"subagent","resource":"general","effect":"allow"}));
        assert!(
            check(&s, &l, &d)
                .unwrap()
                .join("\n")
                .contains("subagent general is allow, expected deny")
        );
    }

    #[test]
    fn stricter_server_rules_are_allowed() {
        let (d, l, mut s) = fixture();
        s.agents[0]["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"action":"edit","resource":"/r/docs/a.md","effect":"deny"}));
        assert_eq!(check(&s, &l, &d).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn server_decides_unlisted_actions() {
        let (d, l, mut s) = fixture();
        for agent in s.agents.as_array_mut().unwrap() {
            let rules = agent["permissions"].as_array_mut().unwrap();
            rules.push(json!({"action":"custom_allowed","resource":"*","effect":"allow"}));
            rules.push(json!({"action":"custom_denied","resource":"*","effect":"deny"}));
        }
        assert!(check(&s, &l, &d).unwrap().is_empty());
    }

    #[test]
    fn lead_mode_is_checked_with_rebuild_hint() {
        let (d, l, mut s) = fixture();
        s.agents[0]["mode"] = json!("subagent");
        let text = check(&s, &l, &d).unwrap().join("\n");
        assert!(text.contains(
            "engineering/ba: lead mode must be primary; run `crewii engineering` to rebuild"
        ));
    }

    #[test]
    fn agent_failures_are_capped_with_exact_count() {
        let (d, l, mut s) = fixture();
        s.agents[0]["mode"] = json!("subagent");
        let rules = s.agents[0]["permissions"].as_array_mut().unwrap();
        for i in 0..20 {
            rules.push(json!({"action":format!("custom{i:02}"),"resource":"*","effect":"ask"}));
        }
        let failures = check(&s, &l, &d).unwrap();
        let agent: Vec<_> = failures
            .iter()
            .filter(|s| s.starts_with("engineering/ba:") || s.ends_with(" for engineering/ba"))
            .collect();
        assert_eq!(agent.len(), 11, "{agent:?}");
        assert!(agent[0].contains("lead mode"));
        assert_eq!(agent[10], "… 11 more for engineering/ba");
    }
}
