//! Division state reader, worktree listing, and status text.
use crate::Result;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
pub struct State {
    pub instances: BTreeMap<String, Instance>,
    pub tickets: BTreeMap<String, Ticket>,
}
#[derive(Deserialize)]
pub struct Instance {
    pub role: String,
    #[serde(rename = "sessionID", default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub active: Option<u64>,
}
#[derive(Deserialize)]
pub struct Ticket {
    pub id: String,
    #[serde(rename = "requesterName")]
    pub requester_name: String,
    pub holder: String,
    pub status: String,
    pub title: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub urgent: bool,
    #[serde(default)]
    pub closed: Option<u64>,
    #[serde(default)]
    pub reason: Option<String>,
}
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub ticket: String,
    pub dirty: bool,
    pub missing: bool,
}
pub fn read(path: &Path) -> Result<Option<State>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| {
            format!(
                "Invalid state file {}: {e}; fix or remove the file",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "Cannot read state file {}: {e}; check file access",
            path.display()
        )),
    }
}

fn git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    crate::git()
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| {
            format!(
                "Cannot run git in {}: {e}; install git and retry",
                root.display()
            )
        })
}

pub fn worktrees(root: &Path, division: &str) -> Result<Vec<Worktree>> {
    let probe = git(root, &["rev-parse", "--is-inside-work-tree"])?;
    if !probe.status.success() || !probe.stdout.starts_with(b"true") {
        let error = String::from_utf8_lossy(&probe.stderr);
        if error.contains("not a git repository") {
            return Ok(vec![]);
        }
        return Err(format!(
            "Cannot inspect repository {}: {}; check repository access",
            root.display(),
            error.trim()
        ));
    }
    let output = git(root, &["worktree", "list", "--porcelain"])?;
    if !output.status.success() {
        return Err(format!(
            "Cannot list worktrees in {}: {}; check git worktree state",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let dir = crate::paths::Layout::new(root.to_path_buf(), division).worktrees_dir();
    let canonical_dir = match dir.canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => root
            .canonicalize()
            .map_err(|e| {
                format!(
                    "Cannot access {}: {e}; check directory access",
                    root.display()
                )
            })?
            .join(".opencode/crewii/worktrees"),
        Err(e) => {
            return Err(format!(
                "Cannot access {}: {e}; check directory access",
                dir.display()
            ));
        }
    };
    let prefix = format!("{division}-");
    let mut result = Vec::new();
    for record in output
        .stdout
        .split(|&b| b == b'\n')
        .collect::<Vec<_>>()
        .split(|field| field.is_empty())
    {
        let Some(raw_path) = record
            .iter()
            .find_map(|field| field.strip_prefix(b"worktree "))
        else {
            continue;
        };
        if raw_path.starts_with(b"\"") {
            continue;
        }
        #[cfg(unix)]
        let path = {
            use std::os::unix::ffi::OsStrExt;
            PathBuf::from(std::ffi::OsStr::from_bytes(raw_path))
        };
        #[cfg(not(unix))]
        let path = PathBuf::from(String::from_utf8_lossy(raw_path).into_owned());
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(ticket) = name.strip_prefix(&prefix) else {
            continue;
        };
        if !ticket
            .strip_prefix("T-")
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        if path
            .parent()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
            .as_deref()
            != Some(canonical_dir.as_path())
        {
            continue;
        }
        let branch = record
            .iter()
            .find_map(|field| field.strip_prefix(b"branch refs/heads/"))
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_else(|| "(detached)".to_owned());
        let missing = !path.exists();
        let dirty = if missing {
            false
        } else {
            let status = git(&path, &["--no-optional-locks", "status", "--porcelain"])?;
            if !status.status.success() {
                return Err(format!(
                    "Cannot inspect worktree {}: {}; repair or prune the worktree",
                    path.display(),
                    String::from_utf8_lossy(&status.stderr).trim()
                ));
            }
            !status.stdout.is_empty()
        };
        result.push(Worktree {
            ticket: ticket.to_owned(),
            path: path.clone(),
            branch,
            dirty,
            missing,
        });
    }
    Ok(result)
}

pub fn status_text(
    division: &str,
    state: Option<&State>,
    wts: &[Worktree],
    now_ms: u64,
    stale_minutes: u32,
) -> String {
    fn numeric(id: &str) -> Option<&str> {
        id.strip_prefix("T-")
            .filter(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
            .map(|s| {
                let n = s.trim_start_matches('0');
                if n.is_empty() { "0" } else { n }
            })
    }
    let Some(state) = state else {
        return format!("{division}: no state yet (open it with: crewii {division})");
    };
    let mut lines = vec![division.to_owned()];
    for (name, instance) in &state.instances {
        let mut tickets: Vec<_> = state
            .tickets
            .values()
            .filter(|ticket| ticket.holder == *name && ticket.status == "open")
            .collect();
        tickets.sort_by(|a, b| match (numeric(&a.id), numeric(&b.id)) {
            (Some(a), Some(b)) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.id.cmp(&b.id),
        });
        lines.push(match tickets.first() {
            Some(ticket) => {
                let title: String = ticket.title.chars().filter(|c| !c.is_control()).collect();
                let extra = if tickets.len() > 1 {
                    format!(" (+{} more)", tickets.len() - 1)
                } else {
                    String::new()
                };
                let activity = instance
                    .active
                    .map(|active| {
                        let minutes = now_ms.saturating_sub(active) / 60_000;
                        let stale = if stale_minutes > 0 && minutes >= u64::from(stale_minutes) {
                            " stale"
                        } else {
                            ""
                        };
                        format!(", active {minutes}m ago{stale}")
                    })
                    .unwrap_or_default();
                format!(
                    "  {name}  {} open {}m{activity}: {title}{extra}",
                    ticket.id,
                    now_ms.saturating_sub(ticket.created) / 60_000
                )
            }
            None => format!("  {name}  idle"),
        });
    }
    let role = |ticket: &Ticket| -> String {
        if !ticket.role.is_empty() {
            return ticket.role.clone();
        }
        match ticket.holder.rsplit_once('-') {
            Some((role, suffix))
                if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) =>
            {
                role.to_owned()
            }
            _ => ticket.holder.clone(),
        }
    };
    let clean = |s: &str| -> String { s.chars().filter(|c| !c.is_control()).collect() };
    let mut backlog: Vec<_> = state
        .tickets
        .values()
        .filter(|t| t.status == "backlog")
        .collect();
    backlog.sort_by_key(|t| (!t.urgent, t.created));
    if !backlog.is_empty() {
        lines.push(format!("backlog ({}):", backlog.len()));
        for t in backlog {
            lines.push(format!(
                "  {}  {}{}  waiting {}m  from {}  {}",
                clean(&t.id),
                clean(&role(t)),
                if t.urgent { "  urgent" } else { "" },
                now_ms.saturating_sub(t.created) / 60_000,
                clean(&t.requester_name),
                clean(&t.title)
            ));
        }
    }
    let mut cancelled: Vec<_> = state
        .tickets
        .values()
        .filter(|t| t.status == "cancelled")
        .collect();
    cancelled.sort_by_key(|t| std::cmp::Reverse(t.closed));
    if !cancelled.is_empty() {
        lines.push("cancelled (last 5):".to_owned());
        for t in cancelled.into_iter().take(5) {
            lines.push(format!(
                "  {}  {}  {}",
                clean(&t.id),
                clean(&role(t)),
                clean(t.reason.as_deref().unwrap_or("-"))
            ));
        }
    }
    let mut closed: Vec<_> = state
        .tickets
        .values()
        .filter(|t| t.status == "closed")
        .collect();
    closed.sort_by_key(|t| std::cmp::Reverse(t.closed));
    if !closed.is_empty() {
        lines.push("closed (last 5):".to_owned());
        for t in closed.into_iter().take(5) {
            lines.push(format!(
                "  {}  {}  done in {}m  {}",
                clean(&t.id),
                clean(&t.holder),
                t.closed.unwrap_or(t.created).saturating_sub(t.created) / 60_000,
                clean(&t.title)
            ));
        }
    }
    lines.push("worktrees:".to_owned());
    for wt in wts {
        lines.push(format!(
            "  {}  {}  {}{}",
            wt.ticket,
            wt.branch,
            wt.path.display(),
            if wt.missing {
                " (missing)"
            } else if wt.dirty {
                " (uncommitted)"
            } else {
                ""
            }
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn git_forces_c_locale() {
        assert_eq!(
            crate::git()
                .get_envs()
                .find(|(key, _)| *key == "LC_ALL")
                .unwrap()
                .1,
            Some(std::ffi::OsStr::new("C"))
        );
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = crate::git().arg("-C").arg(dir).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn sample() -> State {
        serde_json::from_str(r#"{"instances":{"reviewer-02":{"role":"reviewer","sessionID":"ses_2"},"writer-01":{"role":"writer","sessionID":"ses_1"}},"tickets":{"T-3":{"id":"T-3","requesterName":"lead-01","holder":"reviewer-02","status":"open","title":"Review changes"},"T-4":{"id":"T-4","requesterName":"lead-01","holder":"writer-01","status":"closed","title":"Finished"}},"version":1}"#).unwrap()
    }
    #[test]
    fn status_shows_activity_and_stale() {
        let state: State = serde_json::from_value(serde_json::json!({
            "instances": {"backend-01": {"role":"backend", "active": 60000}},
            "tickets": {"T-1": {"id":"T-1", "requesterName":"ba", "holder":"backend-01", "status":"open", "title":"work", "created":0}}
        })).unwrap();
        let text = status_text("eng", Some(&state), &[], 960000, 15);
        assert!(
            text.contains("backend-01  T-1 open 16m, active 15m ago stale: work"),
            "{text}"
        );
        assert!(status_text("eng", Some(&state), &[], 960000, 0).contains("active 15m ago: work"));
        assert!(status_text("eng", Some(&state), &[], 0, 15).contains("active 0m ago: work"));
    }
    #[test]
    fn status_shows_age_cycle_time_and_urgent_first() {
        let state: State = serde_json::from_value(serde_json::json!({
            "instances": {"backend-01": {"role":"backend"}},
            "tickets": {
                "T-1": {"id":"T-1","requesterName":"ba","holder":"backend-01","status":"open","title":"active","created":60000},
                "T-2": {"id":"T-2","requesterName":"ba","holder":"","role":"backend","status":"backlog","title":"normal","created":0},
                "T-3": {"id":"T-3","requesterName":"ba","holder":"","role":"backend","status":"backlog","title":"priority","created":60000,"urgent":true},
                "T-4": {"id":"T-4","requesterName":"ba","holder":"backend-01","status":"closed","title":"done","created":60000,"closed":180000}
            }
        })).unwrap();
        let text = status_text("eng", Some(&state), &[], 240000, 15);
        assert!(text.contains("backend-01  T-1 open 3m: active"), "{text}");
        assert!(text.find("T-3  backend  urgent").unwrap() < text.find("T-2  backend").unwrap());
        assert!(text.contains("closed (last 5):\n  T-4  backend-01  done in 2m  done"));
        assert!(!state.tickets["T-2"].urgent);
    }
    #[test]
    fn status_closed_most_recent_five_and_saturating_times() {
        let mut state = sample();
        state.tickets.clear();
        for n in 1..=6 {
            let ticket: Ticket = serde_json::from_value(serde_json::json!({
                "id": format!("T-{n}"), "requesterName": "ba", "holder": "backend-01",
                "status": "closed", "title": "done", "created": 120000, "closed": n * 60000
            }))
            .unwrap();
            state.tickets.insert(ticket.id.clone(), ticket);
        }
        let text = status_text("eng", Some(&state), &[], 0, 15);
        assert!(!text.contains("T-1  backend-01"));
        assert!(
            text.find("T-6  backend-01  done in 4m").unwrap()
                < text.find("T-2  backend-01  done in 0m").unwrap()
        );
    }
    #[test]
    fn status_backlog_and_cancelled_follow_instances_and_precede_worktrees() {
        let state: State = serde_json::from_value(serde_json::json!({
            "instances": {"backend-02": {"role":"backend"}},
            "tickets": {
                "T-1": {"id":"T-1","requesterName":"ba-01","holder":"backend-02","status":"backlog","title":"old","queued":true},
                "T-2": {"id":"T-2","requesterName":"ba-01","holder":"","role":"qa","status":"backlog","title":"new","created":120000},
                "T-3": {"id":"T-3","requesterName":"ba-01","holder":"backend-02","status":"cancelled","title":"x","closed":1},
                "T-4": {"id":"T-4","requesterName":"ba-01","holder":"","role":"qa","status":"cancelled","title":"x","closed":2,"reason":"obsolete"}
            }
        })).unwrap();
        let text = status_text("eng", Some(&state), &[], 180_000, 15);
        let expected = [
            "backend-02  idle",
            "backlog (2):",
            "T-1  backend  waiting",
            "T-2  qa  waiting",
            "cancelled (last 5):",
            "T-4  qa  obsolete",
            "T-3  backend  -",
            "worktrees:",
        ];
        let mut pos = 0;
        for item in expected {
            pos += text[pos..].find(item).unwrap() + item.len();
        }
        assert!(text.contains("T-1  backend  waiting 3m  from ba-01  old"));
        assert!(text.contains("T-2  qa  waiting 1m  from ba-01  new"));
        assert!(status_text("eng", Some(&state), &[], 1, 15).contains("T-2  qa  waiting 0m"));
    }

    #[test]
    fn status_limits_cancelled_to_five_most_recent() {
        let mut state = sample();
        state.tickets.clear();
        for n in 1..=6 {
            let t: Ticket = serde_json::from_value(serde_json::json!({
                "id":format!("T-{n}"), "requesterName":"ba", "holder":"role-with-dash-01",
                "status":"cancelled", "title":"", "closed":n
            }))
            .unwrap();
            state.tickets.insert(t.id.clone(), t);
        }
        let text = status_text("eng", Some(&state), &[], 0, 15);
        assert!(!text.contains("T-1  role-with-dash"));
        assert!(text.contains("T-6  role-with-dash  -"));
    }
    #[test]
    fn read_missing_none() {
        let t = tempfile::tempdir().unwrap();
        assert!(read(&t.path().join("missing.json")).unwrap().is_none());
    }
    #[test]
    fn read_corrupt_err() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("bad state.json");
        for data in ["{", "{}", "{\"instances\":{},\"tickets\":[]}"] {
            fs::write(&p, data).unwrap();
            assert!(read(&p).err().unwrap().contains("bad state.json"));
        }
        assert!(read(t.path()).is_err());
    }
    #[test]
    fn read_ignores_unknown_fields() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("state.json");
        fs::write(&p, r#"{"instances":{"reviewer-02":{"role":"reviewer","sessionID":"ses_2","extra":true}},"tickets":{"T-3":{"id":"T-3","requesterName":"lead-01","holder":"reviewer-02","status":"open","title":"Review","queued":false}},"version":1}"#).unwrap();
        let s = read(&p).unwrap().unwrap();
        assert_eq!(
            s.instances["reviewer-02"].session_id.as_deref(),
            Some("ses_2")
        );
        assert_eq!(s.tickets["T-3"].requester_name, "lead-01");
    }
    #[test]
    fn read_null_and_missing_sessions() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("state.json");
        fs::write(&p, r#"{"instances":{"a":{"role":"lead","sessionID":null},"b":{"role":"writer"}},"tickets":{}}"#).unwrap();
        let state = read(&p).unwrap().unwrap();
        assert!(state.instances["a"].session_id.is_none());
        assert!(state.instances["b"].session_id.is_none());
    }
    #[test]
    fn worktrees_lists_only_division() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("repo space");
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-b", "test-base"]);
        fs::write(root.join("base"), "base").unwrap();
        git(&root, &["add", "base"]);
        git(
            &root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "initial",
            ],
        );
        assert!(worktrees(&root, "engineering").unwrap().is_empty());
        let dir = root.join(".opencode/crewii/worktrees");
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("engineering-T-1");
        let b = dir.join("sales-T-2");
        let foreign = root.join("engineering-T-9");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "crewii/engineering-T-1-x",
                a.to_str().unwrap(),
            ],
        );
        git(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "foreign",
                foreign.to_str().unwrap(),
            ],
        );
        git(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "crewii/sales-T-2-x",
                b.to_str().unwrap(),
            ],
        );
        let w = worktrees(&root, "engineering").unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].path.canonicalize().unwrap(), a.canonicalize().unwrap());
        assert_eq!(w[0].ticket, "T-1");
        assert_eq!(w[0].branch, "crewii/engineering-T-1-x");
        assert!(!w[0].dirty);
        assert!(status_text("engineering", Some(&sample()), &w, 0, 15).contains("engineering-T-1"));
        fs::write(a.join("changed"), "dirty").unwrap();
        assert!(worktrees(&root, "engineering").unwrap()[0].dirty);
        git(&a, &["checkout", "--detach"]);
        assert_eq!(
            worktrees(&root, "engineering").unwrap()[0].branch,
            "(detached)"
        );
        let gitfile = fs::read(a.join(".git")).unwrap();
        fs::write(a.join(".git"), "invalid gitdir").unwrap();
        assert!(worktrees(&root, "engineering").is_err());
        fs::write(a.join(".git"), gitfile).unwrap();
        fs::rename(&a, dir.join("moved-worktree")).unwrap();
        let missing = worktrees(&root, "engineering").unwrap();
        assert_eq!(missing.len(), 1);
        assert!(
            status_text("engineering", Some(&sample()), &missing, 0, 15).contains(" (missing)")
        );
    }

    #[cfg(unix)]
    #[test]
    fn git_command_and_worktree_directory_errors() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;
        let invalid = Path::new(std::ffi::OsStr::from_bytes(b"bad\0path"));
        assert!(worktrees(invalid, "engineering").is_err());
        let t = tempfile::tempdir().unwrap();
        git(t.path(), &["init"]);
        let crew = t.path().join(".opencode/crewii");
        fs::create_dir_all(&crew).unwrap();
        symlink("worktrees", crew.join("worktrees")).unwrap();
        assert!(worktrees(t.path(), "engineering").is_err());
    }
    #[test]
    fn worktrees_outside_git_empty() {
        let t = tempfile::tempdir().unwrap();
        assert!(worktrees(t.path(), "engineering").unwrap().is_empty());
        assert!(worktrees(&t.path().join("missing"), "engineering").is_err());
    }
    #[test]
    fn status_tickets_numerical_order_and_safe_title() {
        let mut state = sample();
        state.tickets.clear();
        for (id, title) in [("T-10", "later"), ("T-9", "first\n\u{1b}[31m")] {
            state.tickets.insert(
                id.into(),
                Ticket {
                    id: id.into(),
                    requester_name: "lead".into(),
                    holder: "reviewer-02".into(),
                    status: "open".into(),
                    title: title.into(),
                    role: String::new(),
                    created: 0,
                    urgent: false,
                    closed: None,
                    reason: None,
                },
            );
        }
        assert!(
            status_text("eng", Some(&state), &[], 180_000, 15)
                .contains("reviewer-02  T-9 open 3m: first[31m (+1 more)")
        );
        for id in ["T-0000", "invalid", "other"] {
            state.tickets.insert(
                id.into(),
                Ticket {
                    id: id.into(),
                    requester_name: "lead".into(),
                    holder: "reviewer-02".into(),
                    status: "open".into(),
                    title: "title".into(),
                    role: String::new(),
                    created: 0,
                    urgent: false,
                    closed: None,
                    reason: None,
                },
            );
        }
        assert!(
            status_text("eng", Some(&state), &[], 180_000, 15)
                .contains("T-0000 open 3m: title (+4 more)")
        );
    }
    #[test]
    fn worktrees_exact_ticket_and_quoted_paths() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("repo space");
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-b", "base"]);
        git(
            &root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        let dir = root.join(".opencode/crewii/worktrees");
        fs::create_dir_all(&dir).unwrap();
        for (name, branch) in [
            ("eng-T-7", "a"),
            ("engineering-T-8", "b"),
            ("eng-x-T-7", "c"),
            ("eng-T-7x", "d"),
        ] {
            git(
                &root,
                &[
                    "worktree",
                    "add",
                    "-b",
                    branch,
                    dir.join(name).to_str().unwrap(),
                ],
            );
        }
        let found = worktrees(&root, "eng").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].ticket, "T-7");
        #[cfg(unix)]
        {
            let quoted_root = t.path().join("quoted\nrepo");
            fs::create_dir_all(&quoted_root).unwrap();
            git(&quoted_root, &["init", "-b", "base"]);
            git(
                &quoted_root,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "init",
                ],
            );
            let quoted_path = quoted_root.join(".opencode/crewii/worktrees/eng-T-1");
            git(
                &quoted_root,
                &[
                    "worktree",
                    "add",
                    "-b",
                    "other",
                    quoted_path.to_str().unwrap(),
                ],
            );
            assert!(worktrees(&quoted_root, "eng").unwrap().is_empty());
        }
    }
    #[test]
    fn status_busy_idle_and_worktrees() {
        let wt = Worktree {
            path: "/a b".into(),
            branch: "branch".into(),
            ticket: "T-3".into(),
            dirty: true,
            missing: false,
        };
        assert_eq!(
            status_text("engineering", Some(&sample()), &[wt], 0, 15),
            "engineering\n  reviewer-02  T-3 open 0m: Review changes\n  writer-01  idle\nclosed (last 5):\n  T-4  writer-01  done in 0m  Finished\nworktrees:\n  T-3  branch  /a b (uncommitted)"
        );
    }
    #[test]
    fn status_no_state() {
        assert_eq!(
            status_text("engineering", None, &[], 0, 15),
            "engineering: no state yet (open it with: crewii engineering)"
        );
    }
}
