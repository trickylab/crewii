use crate::{
    Result, build, check,
    cli::Cmd,
    models,
    opencode::{self, Api},
    paths::{self, Layout},
    preset, render, state, upstream,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[allow(clippy::type_complexity)]
pub struct Env {
    pub cwd: PathBuf,
    pub var: Box<dyn Fn(&str) -> Option<String>>,
    pub api: Box<dyn Api>,
    pub opencode: PathBuf,
    pub upstream: (String, String),
    pub confirm: Box<dyn Fn(&str) -> bool>,
    pub ask: Box<dyn Fn(&str) -> Option<String>>,
    pub interactive: bool,
}
pub enum Outcome {
    Done(String),
    Launch {
        dir: PathBuf,
        env: Vec<(String, String)>,
    },
}

fn layout(root: &Path, name: &str) -> Result<Layout> {
    let name = preset::resolve_name(name).ok_or_else(|| {
        format!(
            "Unknown division '{name}'; choose: {}",
            preset::builtin_names().join(", ")
        )
    })?;
    Ok(Layout::new(root.to_path_buf(), name))
}
fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "Cannot read {}: {e}; check permissions",
            path.display()
        )),
    }
}
fn load(layout: &Layout) -> Result<preset::Division> {
    preset::load(
        &layout.division,
        read_optional(&layout.override_file())?.as_deref(),
    )
}
fn built(root: &Path, selected: Option<String>) -> Result<Vec<Layout>> {
    if let Some(name) = selected {
        let l = layout(root, &name)?;
        if !l.division_dir().is_dir() {
            return Err(format!("Division not built; run crewii {}", l.division));
        }
        return Ok(vec![l]);
    }
    Ok(preset::builtin_names()
        .into_iter()
        .map(|n| Layout::new(root.to_path_buf(), n))
        .filter(|l| l.division_dir().is_dir())
        .collect())
}
fn model_values(
    env: &Env,
    root: &Path,
) -> Result<(Option<models::Models>, Option<models::Models>)> {
    Ok((
        models::read_local(&models::local_path(root))?,
        models::read_global(&models::global_path(&*env.var)?)?,
    ))
}
fn setup_models(
    env: &Env,
    root: &Path,
    local: Option<&models::Models>,
    global: Option<&models::Models>,
) -> Result<models::Models> {
    let catalog = models::catalog(&*env.api, root)?;
    let current = models::effective(local, global);
    let chosen = models::Models {
        primary: models::pick(
            &catalog,
            "Primary (lead)",
            current.primary.as_deref(),
            &*env.ask,
        ),
        subagent: models::pick(
            &catalog,
            "Subagent (worker)",
            current.subagent.as_deref(),
            &*env.ask,
        ),
    };
    models::write_local(&models::local_path(root), &chosen)?;
    if (env.ask)("Save as global default for new directories? [y/N]: ")
        .is_some_and(|s| s.eq_ignore_ascii_case("y") || s.eq_ignore_ascii_case("yes"))
    {
        models::write_global(&models::global_path(&*env.var)?, &chosen)?;
    }
    Ok(chosen)
}
fn model_line(name: &str, local: Option<&String>, global: Option<&String>, doctor: bool) -> String {
    fn present(v: Option<&String>) -> Option<&String> {
        v.filter(|s| !s.trim().is_empty())
    }
    let (value, source) = if let Some(v) = present(local) {
        (Some(v), "directory")
    } else if let Some(v) = present(global) {
        (Some(v), "global")
    } else {
        (None, "OpenCode default")
    };
    match (value, doctor) {
        (Some(v), true) => format!("ok {name}: {v} ({source})"),
        (None, true) => format!(
            "warn {name}: unset; OpenCode default applies (may be a quota-limited free model); run crewii models"
        ),
        (Some(v), false) => format!("{name}: {v} ({source})"),
        (None, false) => format!("{name}: OpenCode default (OpenCode default)"),
    }
}
fn model_lines(
    local: Option<&models::Models>,
    global: Option<&models::Models>,
    doctor: bool,
) -> String {
    [
        model_line(
            "primary",
            local.and_then(|m| m.primary.as_ref()),
            global.and_then(|m| m.primary.as_ref()),
            doctor,
        ),
        model_line(
            "subagent",
            local.and_then(|m| m.subagent.as_ref()),
            global.and_then(|m| m.subagent.as_ref()),
            doctor,
        ),
    ]
    .join("\n")
}
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn git(root: &Path, args: &[&std::ffi::OsStr]) -> Result<()> {
    let out = crate::git()
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("Cannot run git: {e}; install git"))?;
    if !out.status.success() {
        return Err(format!(
            "Git worktree operation failed: {}; inspect worktrees",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}
fn worktree_remove_args(path: &Path) -> [&std::ffi::OsStr; 3] {
    ["worktree".as_ref(), "remove".as_ref(), path.as_os_str()]
}
// ponytail: walk all files on each status call; cache if huge worktrees make this slow.
fn storage_size(path: &Path) -> Result<u64> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("Cannot inspect {}: {e}", path.display())),
    };
    if meta.is_file() {
        return Ok(meta.len());
    }
    if !meta.is_dir() {
        return Ok(0);
    }
    let entries = fs::read_dir(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut size = 0u64;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        size = size.saturating_add(storage_size(&entry.path())?);
    }
    Ok(size)
}
fn human_size(size: u64) -> String {
    let mut value = size as f64;
    for unit in ["KB", "MB", "GB"] {
        if value < 1024.0 {
            return if unit == "KB" {
                format!("{size} B")
            } else {
                format!("{value:.1} {}", if unit == "MB" { "KB" } else { "MB" })
            };
        }
        value /= 1024.0;
    }
    format!("{value:.1} GB")
}
fn remove_tmp(
    layout: &Layout,
    state: Option<&state::State>,
    prune: bool,
    env: &Env,
) -> Result<usize> {
    let dir = layout.tmp_dir();
    build::no_symlinks(&layout.root, std::slice::from_ref(&dir))?;
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("Cannot read {}: {e}", dir.display())),
    };
    let prefix = format!("{}-", layout.division);
    let mut removed = 0;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Cannot read {}: {e}", dir.display()))?;
        let name = entry.file_name();
        let Some(ticket) = name.to_str().and_then(|name| name.strip_prefix(&prefix)) else {
            continue;
        };
        if prune {
            let valid = ticket
                .strip_prefix("T-")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if !valid
                || !state
                    .and_then(|s| s.tickets.get(ticket))
                    .is_some_and(|t| matches!(t.status.as_str(), "closed" | "cancelled"))
                || !(env.confirm)(&format!(
                    "Remove temporary directory {}?",
                    entry.path().display()
                ))
            {
                continue;
            }
        }
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| format!("Cannot inspect {}: {e}", path.display()))?;
        if meta.is_symlink() {
            fs::remove_file(&path)
        } else if meta.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            continue;
        }
        .map_err(|e| format!("Cannot remove {}: {e}", path.display()))?;
        removed += 1;
    }
    Ok(removed)
}
fn remove_worktrees(
    layout: &Layout,
    state: Option<&state::State>,
    closed_only: bool,
    env: &Env,
    errors: &mut Vec<String>,
) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for wt in state::worktrees(&layout.root, &layout.division)? {
        if closed_only
            && !state
                .and_then(|s| s.tickets.get(&wt.ticket))
                .is_some_and(|t| t.status == "closed")
        {
            continue;
        }
        if wt.dirty && !wt.missing {
            lines.push(format!("dirty: {} (kept)", wt.path.display()));
            continue;
        }
        if wt.branch == "(detached)" {
            lines.push(format!("detached: {} (kept)", wt.path.display()));
            continue;
        }
        if closed_only && !(env.confirm)(&format!("Remove worktree {}?", wt.path.display())) {
            continue;
        }
        if let Err(e) = git(&layout.root, &worktree_remove_args(&wt.path)) {
            if wt.missing {
                lines.push(format!(
                    "missing: {} (run git worktree prune)",
                    wt.path.display()
                ));
            } else {
                errors.push(format!("{}: {e}", wt.path.display()));
            }
            continue;
        }
        lines.push(format!("removed worktree {}", wt.path.display()));
    }
    Ok(lines)
}
fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "Cannot remove {}: {e}; check permissions",
            path.display()
        )),
    }
}
fn remove_empty_dir(path: &Path) -> Result<()> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(e) => Err(format!(
            "Cannot remove {}: {e}; check permissions",
            path.display()
        )),
    }
}

pub fn run(cmd: Cmd, env: &Env) -> Result<Outcome> {
    let root = paths::find_root(&env.cwd);
    let result = match cmd {
        Cmd::Models {
            primary,
            subagent,
            global,
        } => {
            let (local, defaults) = model_values(env, &root)?;
            if primary.is_some() || subagent.is_some() {
                let catalog = models::catalog(&*env.api, &root)?;
                for id in [primary.as_deref(), subagent.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    models::validate(&catalog, id)?;
                }
                let mut chosen = if global {
                    defaults.unwrap_or_default()
                } else {
                    local.unwrap_or_default()
                };
                if primary.is_some() {
                    chosen.primary = primary;
                }
                if subagent.is_some() {
                    chosen.subagent = subagent;
                }
                if global {
                    models::write_global(&models::global_path(&*env.var)?, &chosen)?;
                } else {
                    models::write_local(&models::local_path(&root), &chosen)?;
                }
            } else if env.interactive {
                setup_models(env, &root, local.as_ref(), defaults.as_ref())?;
            }
            let (local, defaults) = model_values(env, &root)?;
            let mut output = model_lines(local.as_ref(), defaults.as_ref(), false);
            if !built(&root, None)?.is_empty() {
                output.push_str("\nreopen the crew to apply");
            }
            output
        }
        Cmd::List => {
            let mut lines = Vec::new();
            for name in preset::builtin_names() {
                let l = layout(&root, name)?;
                lines.push(format!(
                    "{name}: {}",
                    if l.division_dir().is_dir() {
                        "built"
                    } else {
                        "not built"
                    }
                ));
            }
            lines.join("\n")
        }
        Cmd::Open { division } => {
            let l = layout(&root, &division)?;
            opencode::check_version(opencode::version(&env.opencode)?)?;
            let d = load(&l)?;
            let (mut local, global) = model_values(env, &root)?;
            if local.is_none() {
                if env.interactive {
                    local = Some(setup_models(env, &root, None, global.as_ref())?);
                } else {
                    eprintln!("models not chosen for this directory; run crewii models");
                }
            }
            let chosen = models::effective(local.as_ref(), global.as_ref());
            let cache = paths::cache_dir(&*env.var)?.join("agency-agents");
            upstream::ensure(&cache, &env.upstream.0, &env.upstream.1)?;
            let rendered = render::render(
                &d,
                chosen.primary.as_deref(),
                chosen.subagent.as_deref(),
                &|p| upstream::persona_at(&cache, &env.upstream.1, p),
            )?;
            build::build(&l, &d, &rendered)?;
            build::ensure_exclude(&root)?;
            return Ok(Outcome::Launch {
                dir: l.division_dir(),
                env: vec![(
                    "OPENCODE_CLI_CONFIG_CONTENT".into(),
                    r#"{"tabs":{"mode":"on","scope":"cwd"}}"#.into(),
                )],
            });
        }
        Cmd::Ask { division, text } => {
            let text = text.join(" ");
            if text.trim().is_empty() {
                return Err("Request text cannot be empty".into());
            }
            let l = layout(&root, &division)?;
            if !l.division_dir().is_dir() {
                return Err(format!(
                    "No built division; open it first: crewii {}",
                    l.division
                ));
            }
            let s = state::read(&l.state_file())?;
            let id = s
                .as_ref()
                .and_then(|s| {
                    s.instances
                        .values()
                        .filter(|i| i.role == "ba")
                        .chain(s.instances.values().filter(|i| i.role != "ba"))
                        .find_map(|i| i.session_id.as_deref())
                })
                .ok_or_else(|| format!("No session; open it first: crewii {}", l.division))?;
            env.api.post(
                &l.division_dir(),
                &format!("/api/session/{}/command", encode(id)),
                &json!({"name":"crew-ask", "text":text}),
            )?;
            format!("sent to the {} crew (see the ba tab)", l.division)
        }
        Cmd::Status { division, prune } => {
            let mut lines = Vec::new();
            let mut errors = Vec::new();
            for l in built(&root, division)? {
                let s = state::read(&l.state_file())?;
                let w = state::worktrees(&root, &l.division)?;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .min(u64::MAX as u128) as u64;
                let stale_minutes = fs::read(l.plugin_dir().join("config.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .and_then(|config| config.pointer("/limits/staleMinutes")?.as_u64())
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(15);
                lines.push(state::status_text(
                    &l.division,
                    s.as_ref(),
                    &w,
                    now_ms,
                    stale_minutes,
                ));
                lines.push(format!(
                    "storage: worktrees {}, tmp {}",
                    human_size(storage_size(&l.worktrees_dir())?),
                    human_size(storage_size(&l.tmp_dir())?)
                ));
                if prune {
                    lines.extend(remove_worktrees(&l, s.as_ref(), true, env, &mut errors)?);
                    let removed = remove_tmp(&l, s.as_ref(), true, env)?;
                    lines.push(format!("removed {removed} temporary directories"));
                }
            }
            if lines.is_empty() {
                lines.push(format!(
                    "no division built in {}; run `crewii engineering`",
                    root.display()
                ));
            }
            if !errors.is_empty() {
                lines.extend(errors);
                return Err(lines.join("\n"));
            }
            lines.join("\n")
        }
        Cmd::Rm { division, yes } => {
            let l = layout(&root, &division)?;
            build::no_symlinks(
                &root,
                &[
                    l.division_dir(),
                    l.state_file(),
                    l.worktrees_dir(),
                    l.tmp_dir(),
                    l.reports_dir(),
                    l.override_file(),
                ],
            )?;
            if !l.division_dir().is_dir() && !l.state_file().exists() {
                return Err(format!("{} is not built here", l.division));
            }
            if !yes
                && !(env.confirm)(&format!(
                    "Remove the {} crew? Deletes its sessions, clean worktrees (branches kept), temporary files, generated files and state; keeps {}.",
                    l.division,
                    l.override_file().display()
                ))
            {
                return Ok(Outcome::Done("cancelled".into()));
            }
            let mut lines = Vec::new();
            let mut errors = Vec::new();
            let mut ids = std::collections::BTreeSet::new();
            match state::read(&l.state_file()) {
                Ok(Some(s)) => {
                    ids.extend(s.instances.values().filter_map(|i| i.session_id.clone()))
                }
                Ok(None) => (),
                Err(e) => lines.push(format!("Warning: {e}; continuing with listed sessions")),
            }
            if l.division_dir().is_dir() {
                let base = format!(
                    "/api/session?directory={}&limit=200",
                    encode(&l.division_dir().to_string_lossy())
                );
                let mut path = base.clone();
                let mut cursors = std::collections::HashSet::new();
                loop {
                    let page = env.api.session_page(&l.division_dir(), &path)?;
                    let sessions = page
                        .as_array()
                        .or_else(|| page.get("data").and_then(Value::as_array))
                        .or_else(|| page.get("items").and_then(Value::as_array))
                        .ok_or("Invalid session list; check OpenCode version")?;
                    for s in sessions {
                        if s.get("parentID").is_none_or(Value::is_null)
                            && s.get("directory")
                                .is_none_or(|d| d.as_str() == l.division_dir().to_str())
                            && s.get("agent")
                                .and_then(Value::as_str)
                                .is_some_and(|a| a.starts_with(&format!("{}/", l.division)))
                        {
                            let id = s
                                .get("id")
                                .and_then(Value::as_str)
                                .ok_or("Invalid session ID; check OpenCode version")?;
                            ids.insert(id.to_owned());
                        }
                    }
                    let cursor = page
                        .pointer("/cursor/next")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty());
                    if let Some(cursor) = cursor {
                        if !cursors.insert(cursor.to_owned()) {
                            return Err("Repeated session cursor; check OpenCode".into());
                        }
                        path = format!("{base}&cursor={}", encode(cursor));
                    } else {
                        if page.is_array() && sessions.len() >= 200 {
                            lines.push(
                                "listed 200 sessions; some may remain (cursor unavailable)".into(),
                            );
                        }
                        break;
                    }
                }
            }
            for id in ids {
                if let Err(e) = env
                    .api
                    .delete(&l.division_dir(), &format!("/api/session/{}", encode(&id)))
                {
                    errors.push(format!("Cannot delete session {id}: {e}"));
                }
            }
            lines.extend(remove_worktrees(&l, None, false, env, &mut errors)?);
            if !errors.is_empty() {
                lines.extend(errors);
                return Err(lines.join("\n"));
            }
            remove_tmp(&l, None, false, env)?;
            let reports = l.reports_dir();
            match fs::read_dir(&reports) {
                Ok(entries) => {
                    for entry in entries {
                        let entry =
                            entry.map_err(|e| format!("Cannot read {}: {e}", reports.display()))?;
                        let name = entry.file_name();
                        if name.to_str().is_some_and(|name| {
                            name.starts_with(&format!("{}-", l.division)) && name.ends_with(".md")
                        }) {
                            remove_file(&entry.path())?;
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(format!("Cannot read {}: {e}", reports.display())),
            }
            match fs::remove_dir_all(l.division_dir()) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(format!("Cannot remove division: {e}; check permissions")),
            }
            remove_file(&l.state_file())?;
            let state_dir = l.crew_dir().join("state");
            let worktrees_dir = l.worktrees_dir();
            for dir in [
                &state_dir,
                &worktrees_dir,
                &l.tmp_dir(),
                &l.reports_dir(),
                &l.crew_dir(),
                &root.join(".opencode"),
            ] {
                remove_empty_dir(dir)?;
            }
            if !state_dir.exists()
                && !worktrees_dir.exists()
                && !l.tmp_dir().exists()
                && !l.reports_dir().exists()
                && built(&root, None)?.is_empty()
            {
                build::remove_exclude(&root)?;
            }
            lines.push(format!("removed {}", l.division));
            lines.join("\n")
        }
        Cmd::Init { division } => {
            let l = layout(&root, &division)?;
            fs::create_dir_all(l.crew_dir())
                .map_err(|e| format!("Cannot create directory: {e}; check permissions"))?;
            use std::io::Write;
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(l.override_file())
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        return format!("{} already exists", l.override_file().display());
                    }
                    format!(
                        "Cannot create {}: {e}; keep or remove existing override",
                        l.override_file().display()
                    )
                })?;
            f.write_all(b"{\"roles\": []}\n")
                .map_err(|e| format!("Cannot write override: {e}; check permissions"))?;
            format!(
                "Created {}; edit roles to customize",
                l.override_file().display()
            )
        }
        Cmd::Doctor => {
            let lines = doctor(env, &root);
            if lines.lines().any(|l| l.starts_with("fail ")) {
                return Err(lines);
            }
            lines
        }
        Cmd::Check { division } => {
            let mut lines = Vec::new();
            let mut failed = false;
            for l in built(&root, division)? {
                let d = load(&l)?;
                let failures = check::check(&*env.api, &l, &d)?;
                if failures.is_empty() {
                    lines.push(check::ok_line(&d));
                } else {
                    failed = true;
                    lines.extend(failures);
                }
            }
            if lines.is_empty() {
                return Err(format!(
                    "no division built in {}; run `crewii engineering`",
                    root.display()
                ));
            }
            if failed {
                return Err(lines.join("\n"));
            }
            lines.join("\n")
        }
    };
    Ok(Outcome::Done(result))
}
fn doctor(env: &Env, root: &Path) -> String {
    let mut lines = Vec::new();
    let mut item = |name: &str, result: Result<String>| match result {
        Ok(detail) => lines.push(format!("ok {name}: {detail}")),
        Err(detail) => lines.push(format!("fail {name}: {detail}")),
    };
    item(
        "opencode",
        opencode::version(&env.opencode).and_then(|v| {
            opencode::check_version(v)?;
            Ok(format!("{}.{}.{}", v.0, v.1, v.2))
        }),
    );
    item(
        "git",
        crate::git()
            .arg("--version")
            .output()
            .map_err(|e| e.to_string())
            .and_then(|o| {
                if o.status.success() {
                    Ok(String::from_utf8_lossy(&o.stdout).trim().into())
                } else {
                    Err("git unavailable".into())
                }
            }),
    );
    item(
        "default model",
        env.api.get(root, "/api/model/default").and_then(|v| {
            if v.is_null() || v.as_object().is_some_and(|o| o.is_empty()) {
                return Err("no default model; configure one in OpenCode".into());
            }
            let id = v
                .get("id")
                .or_else(|| v.get("modelID"))
                .and_then(Value::as_str);
            Ok(match (v.get("providerID").and_then(Value::as_str), id) {
                (Some(provider), Some(id)) => format!("{provider}/{id}"),
                (_, Some(id)) => id.to_owned(),
                _ => v.to_string(),
            })
        }),
    );
    item(
        "cache",
        paths::cache_dir(&*env.var).and_then(|dir| {
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let file = tempfile_probe(&dir)?;
            fs::remove_file(file).map_err(|e| e.to_string())?;
            Ok(dir.display().to_string())
        }),
    );
    match built(root, None) {
        Ok(ls) => {
            for l in ls {
                item(
                    "plugin",
                    env.api.get(&l.division_dir(), "/api/plugin").and_then(|v| {
                        let a = v.as_array().ok_or("invalid plugin list")?;
                        let n = a
                            .iter()
                            .filter(|p| p.get("id").and_then(Value::as_str) == Some("crewii"))
                            .count();
                        if n == 0 {
                            Err(format!(
                                "plugin not loaded; rebuild with `crewii {}`",
                                l.division
                            ))
                        } else if n > 1 {
                            Err("duplicate crewii plugin; remove global copy".into())
                        } else {
                            Ok(format!("{}: {n} crewii", l.division))
                        }
                    }),
                );
            }
        }
        Err(e) => item("plugin", Err(e)),
    }
    match model_values(env, root) {
        Ok((local, global)) => lines.push(model_lines(local.as_ref(), global.as_ref(), true)),
        Err(e) => lines.push(format!("fail models: {e}")),
    }
    lines.join("\n")
}
fn tempfile_probe(dir: &Path) -> Result<PathBuf> {
    for i in 0..100 {
        let path = dir.join(format!(".crewii-probe-{}-{i}", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Cannot create cache probe; remove stale probes".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};
    struct ApiStub {
        calls: Rc<RefCell<Vec<(String, String, Value)>>>,
        agents: Value,
        sessions: Value,
    }
    impl Api for ApiStub {
        fn get(&self, _: &Path, p: &str) -> Result<Value> {
            self.calls
                .borrow_mut()
                .push(("get".into(), p.into(), Value::Null));
            Ok(if p.starts_with("/api/session?") {
                self.sessions.clone()
            } else if p == "/api/agent" {
                self.agents.clone()
            } else if p == "/api/plugin" {
                json!([{ "id":"crewii" }, { "id":"crewii" }])
            } else if p == "/api/model" {
                json!([{ "providerID":"p", "modelID":"m", "name":"Model", "enabled":true, "capabilities":{"tools":true}, "variants":[{"id":"high"}] }])
            } else {
                json!({"model":"fast"})
            })
        }
        fn post(&self, _: &Path, p: &str, b: &Value) -> Result<Value> {
            self.calls
                .borrow_mut()
                .push(("post".into(), p.into(), b.clone()));
            Ok(Value::Null)
        }
        fn delete(&self, _: &Path, p: &str) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(("delete".into(), p.into(), Value::Null));
            Ok(())
        }
    }
    fn env(dir: &Path, calls: Rc<RefCell<Vec<(String, String, Value)>>>, confirm: bool) -> Env {
        let home = dir.to_string_lossy().into_owned();
        Env {
            cwd: dir.to_path_buf(),
            var: Box::new(move |k| (k == "HOME").then(|| home.clone())),
            api: Box::new(ApiStub {
                calls,
                agents: json!([]),
                sessions: json!([{ "id":"root", "agent":"engineering/ba" }, { "id":"child", "agent":"engineering/ba", "parentID":"root" }, { "id":"other", "agent":"other/ba" }]),
            }),
            opencode: "/nonexistent".into(),
            upstream: (upstream::URL.into(), upstream::sha().into()),
            confirm: Box::new(move |_| confirm),
            ask: Box::new(|_| None),
            interactive: false,
        }
    }
    fn done(out: Result<Outcome>) -> String {
        match out.unwrap() {
            Outcome::Done(s) => s,
            _ => panic!("unexpected launch"),
        }
    }
    fn setup() -> (tempfile::TempDir, Layout) {
        let t = tempfile::tempdir().unwrap();
        let l = Layout::new(t.path().to_path_buf(), "engineering");
        (t, l)
    }
    #[test]
    fn model_flags_validate_before_writing_and_preserve_fields() {
        let (t, _) = setup();
        let e = env(t.path(), Rc::new(RefCell::new(vec![])), false);
        for bad in ["bad", "p/m#bad"] {
            assert!(
                run(
                    Cmd::Models {
                        primary: Some(bad.into()),
                        subagent: Some("p/m#high".into()),
                        global: false
                    },
                    &e
                )
                .is_err()
            );
            assert!(!models::local_path(t.path()).exists());
            assert!(!models::global_path(&*e.var).unwrap().exists());
        }
        done(run(
            Cmd::Models {
                primary: None,
                subagent: Some("p/m#high".into()),
                global: true,
            },
            &e,
        ));
        assert!(!models::local_path(t.path()).exists());
        assert_eq!(
            models::read_global(&models::global_path(&*e.var).unwrap())
                .unwrap()
                .unwrap()
                .subagent
                .as_deref(),
            Some("p/m#high")
        );
        done(run(
            Cmd::Models {
                primary: Some("p/m".into()),
                subagent: None,
                global: true,
            },
            &e,
        ));
        let output = done(run(
            Cmd::Models {
                primary: None,
                subagent: None,
                global: false,
            },
            &e,
        ));
        assert!(output.contains("primary: p/m (global)"), "{output}");
        assert!(output.contains("subagent: p/m#high (global)"), "{output}");
        let doctor = doctor(&e, t.path());
        assert!(doctor.contains("ok primary: p/m (global)"), "{doctor}");
        assert!(
            doctor.contains("ok subagent: p/m#high (global)"),
            "{doctor}"
        );
        done(run(
            Cmd::Models {
                primary: Some("p/m#high".into()),
                subagent: None,
                global: false,
            },
            &e,
        ));
        let before = fs::read(models::local_path(t.path())).unwrap();
        assert!(
            run(
                Cmd::Models {
                    primary: Some("p/m#invalid".into()),
                    subagent: None,
                    global: false
                },
                &e
            )
            .is_err()
        );
        assert_eq!(fs::read(models::local_path(t.path())).unwrap(), before);
        let output = done(run(
            Cmd::Models {
                primary: None,
                subagent: None,
                global: false,
            },
            &e,
        ));
        assert!(output.contains("primary: p/m#high (directory)"));
        assert!(output.contains("subagent: p/m#high (global)"));
    }
    #[test]
    fn model_setup_global_and_doctor_unset() {
        let (t, l) = setup();
        let mut e = env(t.path(), Rc::new(RefCell::new(vec![])), false);
        let doctor = doctor(&e, t.path());
        assert!(doctor.contains("warn primary: unset"));
        assert!(doctor.contains("warn subagent: unset"));
        e.interactive = true;
        let answers = Rc::new(RefCell::new(std::collections::VecDeque::from([
            "", "", "y",
        ])));
        e.ask = Box::new(move |_| answers.borrow_mut().pop_front().map(str::to_owned));
        fs::create_dir_all(l.division_dir()).unwrap();
        let output = done(run(
            Cmd::Models {
                primary: None,
                subagent: None,
                global: false,
            },
            &e,
        ));
        assert!(output.contains("reopen the crew to apply"));
        assert!(models::local_path(t.path()).exists());
        assert!(models::global_path(&*e.var).unwrap().exists());
    }
    #[test]
    fn list_init_errors_and_ask() {
        let (t, l) = setup();
        let calls = Rc::new(RefCell::new(vec![]));
        let e = env(t.path(), calls.clone(), false);
        assert!(done(run(Cmd::List, &e)).contains("not built"));
        assert!(
            run(
                Cmd::Open {
                    division: "bad".into()
                },
                &e
            )
            .err()
            .unwrap()
            .contains("engineering")
        );
        assert!(
            run(
                Cmd::Open {
                    division: "eng".into()
                },
                &e
            )
            .is_err()
        );
        assert!(
            done(run(
                Cmd::Init {
                    division: "eng".into()
                },
                &e
            ))
            .contains("Created")
        );
        assert!(
            run(
                Cmd::Init {
                    division: "eng".into()
                },
                &e
            )
            .is_err()
        );
        assert!(
            run(
                Cmd::Ask {
                    division: "eng".into(),
                    text: vec!["hello".into()]
                },
                &e
            )
            .err()
            .unwrap()
            .contains("crewii engineering")
        );
        fs::create_dir_all(l.state_file().parent().unwrap()).unwrap();
        fs::write(
            l.state_file(),
            r#"{"instances":{"ba-01":{"role":"ba","sessionID":"s/1"}},"tickets":{}}"#,
        )
        .unwrap();
        fs::create_dir_all(l.division_dir()).unwrap();
        assert!(
            done(run(
                Cmd::Ask {
                    division: "eng".into(),
                    text: vec!["hello".into()]
                },
                &e
            ))
            .contains("sent")
        );
        assert!(
            calls
                .borrow()
                .iter()
                .any(|(method, path, body)| method == "post"
                    && path == "/api/session/s%2F1/command"
                    && body == &json!({"name":"crew-ask","text":"hello"}))
        );
        assert!(
            done(run(
                Cmd::Rm {
                    division: "eng".into(),
                    yes: false
                },
                &e
            ))
            .contains("cancelled")
        );
        assert!(l.division_dir().exists());
        assert!(l.state_file().exists());
        assert!(!calls.borrow().iter().any(|(m, _, _)| m == "delete"));
        assert!(l.override_file().exists());
    }
    #[test]
    fn status_reads_stale_value_from_plugin_config() {
        let (t, l) = setup();
        let e = env(t.path(), Rc::new(RefCell::new(vec![])), false);
        fs::create_dir_all(l.plugin_dir()).unwrap();
        fs::write(
            l.plugin_dir().join("config.json"),
            r#"{"limits":{"staleMinutes":0}}"#,
        )
        .unwrap();
        fs::create_dir_all(l.state_file().parent().unwrap()).unwrap();
        fs::write(l.state_file(), r#"{"instances":{"ba-01":{"role":"ba","active":0}},"tickets":{"T-1":{"id":"T-1","requesterName":"user","holder":"ba-01","status":"open","title":"work"}}}"#).unwrap();
        let out = done(run(
            Cmd::Status {
                division: Some("eng".into()),
                prune: false,
            },
            &e,
        ));
        assert!(out.contains("active "), "{out}");
        assert!(!out.contains("ago stale"), "{out}");
        for config in [
            r#"{"limits":{"staleMinutes":15}}"#,
            r#"{"limits":{"staleMinutes":"invalid"}}"#,
            "invalid",
        ] {
            fs::write(l.plugin_dir().join("config.json"), config).unwrap();
            let out = done(run(
                Cmd::Status {
                    division: Some("eng".into()),
                    prune: false,
                },
                &e,
            ));
            assert!(out.contains("ago stale"), "{out}");
        }
        fs::remove_file(l.plugin_dir().join("config.json")).unwrap();
        let out = done(run(
            Cmd::Status {
                division: Some("eng".into()),
                prune: false,
            },
            &e,
        ));
        assert!(out.contains("ago stale"), "{out}");
    }
    #[test]
    fn rm_removes_reports() {
        let (t, l) = setup();
        let e = env(t.path(), Rc::new(RefCell::new(vec![])), false);
        fs::create_dir_all(l.division_dir()).unwrap();
        let reports = l.crew_dir().join("reports");
        fs::create_dir_all(&reports).unwrap();
        fs::write(reports.join("engineering-T-1.md"), "a").unwrap();
        fs::write(reports.join("sales-T-1.md"), "b").unwrap();
        done(run(
            Cmd::Status {
                division: Some("eng".into()),
                prune: true,
            },
            &e,
        ));
        assert!(reports.join("engineering-T-1.md").exists());
        done(run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        ));
        assert!(!reports.join("engineering-T-1.md").exists());
        assert!(reports.join("sales-T-1.md").exists());
        fs::create_dir_all(l.division_dir()).unwrap();
        fs::remove_file(reports.join("sales-T-1.md")).unwrap();
        done(run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        ));
        assert!(!reports.exists());
    }
    #[test]
    fn rm_sessions_status_check_doctor() {
        let (t, _) = setup();
        let project = t.path().join("my project é");
        fs::create_dir_all(&project).unwrap();
        let l = Layout::new(project.clone(), "engineering");
        let calls = Rc::new(RefCell::new(vec![]));
        let e = env(&project, calls.clone(), false);
        fs::create_dir_all(l.division_dir()).unwrap();
        fs::write(l.override_file(), "{}").unwrap();
        assert!(
            done(run(
                Cmd::Status {
                    division: None,
                    prune: true
                },
                &e
            ))
            .contains("no state")
        );
        assert!(
            run(
                Cmd::Check {
                    division: Some("eng".into())
                },
                &e
            )
            .err()
            .unwrap()
            .contains("missing agent")
        );
        let d = run(Cmd::Doctor, &e).err().unwrap();
        for expected in [
            "fail opencode",
            "ok git",
            "ok default model",
            "ok cache",
            "fail plugin",
        ] {
            assert!(d.contains(expected), "{d}");
        }
        let output = done(run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        ));
        assert!(output.contains("removed engineering"));
        assert!(!l.division_dir().exists());
        assert!(l.override_file().exists());
        assert_eq!(fs::read_dir(l.crew_dir()).unwrap().count(), 1);
        let c = calls.borrow();
        assert!(c.iter().any(|(_, p, _)| p.contains("directory=%2F")));
        assert!(
            c.iter()
                .any(|(_, p, _)| p.contains("my%20project%20%C3%A9"))
        );
        assert!(
            c.iter()
                .any(|(m, p, _)| m == "delete" && p == "/api/session/root")
        );
        assert!(
            !c.iter()
                .any(|(m, p, _)| m == "delete" && p == "/api/session/child")
        );
        assert!(
            run(
                Cmd::Check {
                    division: Some("eng".into())
                },
                &e
            )
            .is_err()
        );
    }
    fn git_ok(root: &Path, args: &[&str]) {
        let out = crate::git()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    #[cfg(unix)]
    #[test]
    fn open_builds_with_local_fixture() {
        use std::os::unix::fs::PermissionsExt;
        let (t, l) = setup();
        let mut e = env(t.path(), Rc::new(RefCell::new(vec![])), true);
        e.ask = Box::new(|_| panic!("non-interactive open prompted"));
        let fixture = t.path().join("fixture repo");
        fs::create_dir_all(&fixture).unwrap();
        git_ok(&fixture, &["init"]);
        let division = preset::load("engineering", None).unwrap();
        for role in division.roles {
            let path = fixture.join(role.persona);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "---\nname: x\n---\nPersona body\n").unwrap();
        }
        git_ok(&fixture, &["add", "."]);
        git_ok(
            &fixture,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "fixture",
            ],
        );
        let output = crate::git()
            .arg("-C")
            .arg(&fixture)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(output.status.success());
        e.upstream = (
            fixture.to_str().unwrap().into(),
            String::from_utf8(output.stdout).unwrap().trim().into(),
        );
        let bin = t.path().join("opencode");
        fs::write(&bin, "#!/bin/sh\necho opencode v2.2.5\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        e.opencode = bin;
        fs::create_dir_all(l.crew_dir()).unwrap();
        fs::write(l.override_file(), r#"{"roles":[]}"#).unwrap();
        let result = run(
            Cmd::Open {
                division: "eng".into(),
            },
            &e,
        )
        .unwrap();
        match result {
            Outcome::Launch { dir, env } => {
                assert_eq!(dir, l.division_dir());
                assert_eq!(
                    env,
                    vec![(
                        "OPENCODE_CLI_CONFIG_CONTENT".into(),
                        r#"{"tabs":{"mode":"on","scope":"cwd"}}"#.into()
                    )]
                );
            }
            _ => panic!("expected launch"),
        }
        assert!(l.agents_dir().join("ba.md").exists());
        assert!(
            !fs::read_to_string(l.agents_dir().join("worker/qa.md"))
                .unwrap()
                .contains("\nmodel:")
        );
        assert!(!models::local_path(t.path()).exists());
        let prompts = Rc::new(RefCell::new(Vec::new()));
        let recorded = prompts.clone();
        let answers = RefCell::new(std::collections::VecDeque::from([
            "p/", "1", "", "p/m#high", "n",
        ]));
        e.interactive = true;
        e.ask = Box::new(move |prompt| {
            recorded.borrow_mut().push(prompt.to_owned());
            answers.borrow_mut().pop_front().map(str::to_owned)
        });
        run(
            Cmd::Open {
                division: "eng".into(),
            },
            &e,
        )
        .unwrap();
        assert!(models::local_path(t.path()).exists());
        for role in preset::load("engineering", None).unwrap().roles {
            assert!(
                fs::read_to_string(l.agents_dir().join(format!("{}.md", role.id)))
                    .unwrap()
                    .contains("model: \"p/m\"")
            );
            assert!(
                fs::read_to_string(
                    l.agents_dir()
                        .join("worker")
                        .join(format!("{}.md", role.id))
                )
                .unwrap()
                .contains("model: \"p/m#high\"")
            );
        }
        let count = prompts.borrow().len();
        run(
            Cmd::Open {
                division: "eng".into(),
            },
            &e,
        )
        .unwrap();
        assert_eq!(prompts.borrow().len(), count);
        assert!(done(run(Cmd::List, &e)).contains("built"));
        assert!(run(Cmd::Doctor, &e).err().unwrap().contains("ok opencode"));
        fs::write(&e.opencode, "#!/bin/sh\necho opencode v2.2.4\n").unwrap();
        assert!(
            run(
                Cmd::Open {
                    division: "eng".into()
                },
                &e
            )
            .err()
            .unwrap()
            .contains("2.2.5")
        );
    }
    #[test]
    fn storage_status_prune_and_remove() {
        let (t, l) = setup();
        let tmp = l.crew_dir().join("tmp");
        fs::create_dir_all(l.division_dir()).unwrap();
        fs::create_dir_all(l.worktrees_dir()).unwrap();
        fs::write(l.worktrees_dir().join("data"), vec![0; 2048]).unwrap();
        for name in [
            "engineering-T-1",
            "engineering-T-2",
            "engineering-T-3",
            "engineering-T-x",
            "product-T-1",
        ] {
            fs::create_dir_all(tmp.join(name)).unwrap();
            fs::write(tmp.join(name).join("data"), vec![0; 10]).unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(l.worktrees_dir(), tmp.join("linked")).unwrap();
        fs::create_dir_all(l.state_file().parent().unwrap()).unwrap();
        fs::write(l.state_file(), r#"{"instances":{},"tickets":{"T-1":{"id":"T-1","requesterName":"x","holder":"x","status":"closed","title":"x"},"T-2":{"id":"T-2","requesterName":"x","holder":"x","status":"cancelled","title":"x"},"T-3":{"id":"T-3","requesterName":"x","holder":"x","status":"open","title":"x"}}}"#).unwrap();
        let no = env(t.path(), Rc::new(RefCell::new(vec![])), false);
        let output = done(run(
            Cmd::Status {
                division: Some("eng".into()),
                prune: true,
            },
            &no,
        ));
        assert!(
            output.contains("storage: worktrees 2.0 KB, tmp 50 B"),
            "{output}"
        );
        assert!(tmp.join("engineering-T-1").exists());
        let yes = env(t.path(), Rc::new(RefCell::new(vec![])), true);
        let output = done(run(
            Cmd::Status {
                division: Some("eng".into()),
                prune: true,
            },
            &yes,
        ));
        assert!(output.contains("2 temporary"), "{output}");
        assert!(!tmp.join("engineering-T-1").exists());
        assert!(!tmp.join("engineering-T-2").exists());
        assert!(tmp.join("engineering-T-3").exists());
        assert!(tmp.join("engineering-T-x").exists());
        assert!(tmp.join("product-T-1").exists());
        done(run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &yes,
        ));
        assert!(!tmp.join("engineering-T-3").exists());
        assert!(!tmp.join("engineering-T-x").exists());
        assert!(tmp.join("product-T-1").exists());
    }

    #[test]
    fn storage_sizes_and_empty_tmp_cleanup() {
        let (t, l) = setup();
        assert_eq!(storage_size(&l.tmp_dir()).unwrap(), 0);
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(1024 * 1024 * 1024), "1.0 GB");
        fs::create_dir_all(l.division_dir()).unwrap();
        fs::create_dir_all(l.tmp_dir().join("engineering-T-1")).unwrap();
        let e = env(t.path(), Rc::new(RefCell::new(vec![])), true);
        done(run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        ));
        assert!(!l.tmp_dir().exists());
    }

    #[cfg(unix)]
    #[test]
    fn tmp_cleanup_unlinks_symlinks_without_following_them() {
        let (t, l) = setup();
        let outside = t.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep"), "data").unwrap();
        fs::create_dir_all(l.division_dir()).unwrap();
        fs::create_dir_all(l.tmp_dir()).unwrap();
        std::os::unix::fs::symlink(&outside, l.tmp_dir().join("engineering-T-1")).unwrap();
        let e = env(t.path(), Rc::new(RefCell::new(vec![])), true);
        done(run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        ));
        assert!(outside.join("keep").exists());
        assert!(!l.tmp_dir().exists());
    }

    #[test]
    fn prune_closed_and_keep_dirty() {
        let (t, l) = setup();
        let root = t.path();
        let calls = Rc::new(RefCell::new(vec![]));
        let e = env(root, calls, true);
        git_ok(root, &["init"]);
        build::ensure_exclude(root).unwrap();
        git_ok(root, &["config", "user.email", "a@b"]);
        git_ok(root, &["config", "user.name", "a"]);
        fs::write(root.join("file"), "base").unwrap();
        git_ok(root, &["add", "file"]);
        git_ok(
            root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "test",
            ],
        );
        fs::create_dir_all(l.worktrees_dir()).unwrap();
        fs::create_dir_all(l.division_dir()).unwrap();
        fs::create_dir_all(l.state_file().parent().unwrap()).unwrap();
        let a = l.worktrees_dir().join("engineering-T-1");
        let b = l.worktrees_dir().join("engineering-T-2");
        git_ok(
            root,
            &["worktree", "add", "-b", "work-1", a.to_str().unwrap()],
        );
        git_ok(
            root,
            &["worktree", "add", "-b", "work-2", b.to_str().unwrap()],
        );
        fs::write(b.join("file"), "dirty").unwrap();
        fs::write(l.state_file(), r#"{"instances":{},"tickets":{"T-1":{"id":"T-1","requesterName":"x","holder":"x","status":"closed","title":"x"},"T-2":{"id":"T-2","requesterName":"x","holder":"x","status":"closed","title":"x"}}}"#).unwrap();
        let no = env(root, Rc::new(RefCell::new(vec![])), false);
        done(run(
            Cmd::Status {
                division: Some("eng".into()),
                prune: true,
            },
            &no,
        ));
        assert!(a.exists());
        let result = done(run(
            Cmd::Status {
                division: Some("eng".into()),
                prune: true,
            },
            &e,
        ));
        assert!(result.contains("removed worktree"));
        assert!(result.contains("dirty:"));
        assert!(!a.exists());
        assert!(b.exists());
        let result = done(run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        ));
        assert!(result.contains("dirty:"));
        assert!(b.exists());
        assert!(l.worktrees_dir().exists());
        assert!(
            fs::read_to_string(root.join(".git/info/exclude"))
                .unwrap()
                .contains(build::EXCLUDE_LINE)
        );
    }
    #[test]
    fn rm_removes_empty_crew_directories() {
        let (t, l) = setup();
        fs::create_dir_all(l.division_dir()).unwrap();
        fs::create_dir_all(l.worktrees_dir()).unwrap();
        fs::create_dir_all(l.state_file().parent().unwrap()).unwrap();
        fs::write(l.state_file(), r#"{"instances":{},"tickets":{}}"#).unwrap();
        let e = env(t.path(), Rc::new(RefCell::new(vec![])), true);
        done(run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        ));
        assert!(!l.crew_dir().exists());
    }
    #[cfg(unix)]
    #[test]
    fn rm_refuses_symlinked_crew_path() {
        let t = tempfile::tempdir().unwrap();
        let (root, outside) = (t.path().join("my project"), t.path().join("outside"));
        let victim = outside.join("crewii/engineering");
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep"), "x").unwrap();
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".opencode")).unwrap();
        let e = env(&root, Rc::new(RefCell::new(vec![])), true);
        let err = run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        )
        .err()
        .unwrap();
        assert!(err.contains("is a symlink"), "{err}");
        assert!(victim.join("keep").exists());
    }
    #[test]
    fn missing_and_detached_worktrees_are_safe() {
        let (t, l) = setup();
        let root = t.path();
        git_ok(root, &["init"]);
        fs::write(root.join("file"), "base").unwrap();
        git_ok(root, &["add", "file"]);
        git_ok(
            root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "base",
            ],
        );
        fs::create_dir_all(l.worktrees_dir()).unwrap();
        fs::create_dir_all(l.division_dir()).unwrap();
        let missing = l.worktrees_dir().join("engineering-T-1");
        let detached = l.worktrees_dir().join("engineering-T-2");
        let locked_missing = l.worktrees_dir().join("engineering-T-3");
        let unrelated = root.join("user worktree");
        git_ok(
            root,
            &["worktree", "add", "-b", "one", missing.to_str().unwrap()],
        );
        git_ok(
            root,
            &["worktree", "add", "--detach", detached.to_str().unwrap()],
        );
        git_ok(
            root,
            &[
                "worktree",
                "add",
                "-b",
                "locked-missing",
                locked_missing.to_str().unwrap(),
            ],
        );
        git_ok(
            root,
            &["worktree", "lock", locked_missing.to_str().unwrap()],
        );
        git_ok(
            root,
            &["worktree", "add", "-b", "user", unrelated.to_str().unwrap()],
        );
        fs::remove_dir_all(&missing).unwrap();
        fs::remove_dir_all(&locked_missing).unwrap();
        fs::remove_dir_all(&unrelated).unwrap();
        let e = env(root, Rc::new(RefCell::new(vec![])), true);
        let output = done(run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        ));
        assert!(output.contains("detached:"));
        assert!(output.lines().any(|line| line.starts_with("missing: ")
            && line.contains("engineering-T-3")
            && line.contains("run git worktree prune")));
        assert!(output.lines().any(|line| line.contains("engineering-T-1")
            && (line.starts_with("removed worktree ") || line.starts_with("missing: "))));
        assert!(detached.exists());
        let listed = crate::git()
            .arg("-C")
            .arg(root)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&listed.stdout).contains(unrelated.to_str().unwrap()));
    }
    #[test]
    fn failure_paths_and_probe_collision() {
        let (t, l) = setup();
        assert!(read_optional(t.path()).is_err());
        assert!(remove_file(t.path()).is_err());
        assert!(
            git(
                t.path(),
                &["worktree".as_ref(), "remove".as_ref(), "missing".as_ref()]
            )
            .is_err()
        );
        let first = t
            .path()
            .join(format!(".crewii-probe-{}-0", std::process::id()));
        fs::write(&first, "preserve").unwrap();
        assert_ne!(tempfile_probe(t.path()).unwrap(), first);
        assert!(tempfile_probe(&l.division_dir()).is_err());
        for i in 1..100 {
            fs::write(
                t.path()
                    .join(format!(".crewii-probe-{}-{i}", std::process::id())),
                "",
            )
            .unwrap();
        }
        assert!(tempfile_probe(t.path()).is_err());
        let e = env(t.path(), Rc::new(RefCell::new(vec![])), true);
        fs::create_dir_all(l.state_file().parent().unwrap()).unwrap();
        fs::write(l.state_file(), r#"{"instances":{},"tickets":{}}"#).unwrap();
        assert!(
            done(run(
                Cmd::Rm {
                    division: "eng".into(),
                    yes: true
                },
                &e
            ))
            .contains("removed")
        );
    }
    #[test]
    fn round_two_empty_and_unbuilt() {
        let (t, _) = setup();
        let e = env(t.path(), Rc::new(RefCell::new(vec![])), false);
        assert!(
            done(run(
                Cmd::Status {
                    division: None,
                    prune: false
                },
                &e
            ))
            .contains("no division built")
        );
        assert!(
            run(Cmd::Check { division: None }, &e)
                .err()
                .unwrap()
                .contains("no division built")
        );
        assert!(
            run(
                Cmd::Rm {
                    division: "eng".into(),
                    yes: true
                },
                &e
            )
            .err()
            .unwrap()
            .contains("is not built here")
        );
    }
    #[test]
    fn ask_prefers_ba_and_rejects_empty() {
        let (t, l) = setup();
        let calls = Rc::new(RefCell::new(vec![]));
        let e = env(t.path(), calls.clone(), false);
        fs::create_dir_all(l.division_dir()).unwrap();
        fs::create_dir_all(l.state_file().parent().unwrap()).unwrap();
        fs::write(l.state_file(), r#"{"instances":{"a":{"role":"qa","sessionID":"qa"},"z":{"role":"ba","sessionID":"ba"}},"tickets":{}}"#).unwrap();
        assert!(
            run(
                Cmd::Ask {
                    division: "eng".into(),
                    text: vec!["   ".into()]
                },
                &e
            )
            .is_err()
        );
        assert!(
            done(run(
                Cmd::Ask {
                    division: "eng".into(),
                    text: vec!["hello".into()]
                },
                &e
            ))
            .contains("sent")
        );
        assert!(
            calls
                .borrow()
                .iter()
                .any(|(_, p, _)| p == "/api/session/ba/command")
        );
    }

    struct ResponseApi {
        sessions: Value,
        model: Value,
        plugins: Value,
        calls: Rc<RefCell<Vec<String>>>,
    }
    impl Api for ResponseApi {
        fn get(&self, _: &Path, path: &str) -> Result<Value> {
            Ok(if path.starts_with("/api/session?") {
                self.sessions.clone()
            } else if path == "/api/model/default" {
                self.model.clone()
            } else {
                self.plugins.clone()
            })
        }
        fn post(&self, _: &Path, _: &str, _: &Value) -> Result<Value> {
            Ok(Value::Null)
        }
        fn delete(&self, _: &Path, path: &str) -> Result<()> {
            self.calls.borrow_mut().push(path.into());
            if path.ends_with("/bad") {
                Err("delete failed".into())
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn rm_union_and_failures() {
        let (t, l) = setup();
        fs::create_dir_all(l.division_dir()).unwrap();
        fs::create_dir_all(l.state_file().parent().unwrap()).unwrap();
        fs::write(l.state_file(), r#"{"instances":{"a":{"role":"ba","sessionID":"bad"},"b":{"role":"qa","sessionID":"root"},"c":{"role":"qa","sessionID":"state"}},"tickets":{}}"#).unwrap();
        let calls = Rc::new(RefCell::new(vec![]));
        let mut sessions = vec![
            json!({"id":"root","agent":"engineering/ba","directory":l.division_dir()}),
            json!({"id":"wrong","agent":"engineering/ba","directory":"/other"}),
            json!({"id":"child","agent":"engineering/ba","parentID":"root"}),
        ];
        sessions.resize(200, json!({"id":"other","agent":"other"}));
        let mut e = env(t.path(), Rc::new(RefCell::new(vec![])), false);
        e.api = Box::new(ResponseApi {
            sessions: json!(sessions),
            model: Value::Null,
            plugins: json!([]),
            calls: calls.clone(),
        });
        let error = run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        )
        .err()
        .unwrap();
        assert!(error.contains("listed 200 sessions; some may remain"));
        assert!(error.contains("delete failed"));
        assert!(l.state_file().exists());
        assert_eq!(
            &*calls.borrow(),
            &[
                "/api/session/bad",
                "/api/session/root",
                "/api/session/state"
            ]
        );
        let doctor = run(Cmd::Doctor, &e).err().unwrap();
        assert!(doctor.contains("no default model; configure one in OpenCode"));
        assert!(doctor.contains("plugin not loaded; rebuild with `crewii engineering`"));
    }
    #[test]
    fn rm_paginates_and_ignores_corrupt_state() {
        struct Pages(Rc<RefCell<Vec<String>>>);
        impl Api for Pages {
            fn get(&self, _: &Path, path: &str) -> Result<Value> {
                self.0.borrow_mut().push(path.into());
                Ok(if path.contains("cursor=next%2Bpage") {
                    json!({"data":[{"id":"second","agent":"engineering/ba"}]})
                } else {
                    json!({"data":[{"id":"first","agent":"engineering/ba"}],"cursor":{"next":"next+page"}})
                })
            }
            fn post(&self, _: &Path, _: &str, _: &Value) -> Result<Value> {
                unreachable!()
            }
            fn delete(&self, _: &Path, path: &str) -> Result<()> {
                self.0.borrow_mut().push(path.into());
                Ok(())
            }
        }
        let (t, l) = setup();
        git_ok(t.path(), &["init"]);
        fs::create_dir_all(l.division_dir()).unwrap();
        fs::create_dir_all(l.state_file().parent().unwrap()).unwrap();
        fs::write(l.state_file(), "invalid").unwrap();
        let calls = Rc::new(RefCell::new(vec![]));
        let mut e = env(t.path(), Rc::new(RefCell::new(vec![])), true);
        e.api = Box::new(Pages(calls.clone()));
        let result = done(run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        ));
        assert!(result.contains("Warning:"));
        assert!(
            calls
                .borrow()
                .iter()
                .any(|p| p.contains("cursor=next%2Bpage"))
        );
        assert!(calls.borrow().contains(&"/api/session/first".into()));
        assert!(calls.borrow().contains(&"/api/session/second".into()));
        assert!(!t.path().join(".opencode").exists());
    }
    #[test]
    fn worktree_removal_never_forces() {
        assert_eq!(
            worktree_remove_args(Path::new("path")),
            ["worktree", "remove", "path"]
        );
    }
    #[test]
    fn doctor_model_shapes() {
        let (t, l) = setup();
        fs::create_dir_all(l.division_dir()).unwrap();
        let mut e = env(t.path(), Rc::new(RefCell::new(vec![])), true);
        for (model, expected) in [
            (json!({}), "no default model"),
            (json!({"id":"x"}), "default model: x"),
            (
                json!({"modelID":"x","providerID":"p"}),
                "default model: p/x",
            ),
            (json!({"other":3}), "default model: {\"other\":3}"),
        ] {
            e.api = Box::new(ResponseApi {
                sessions: json!([]),
                model,
                plugins: json!([{"id":"crewii"}]),
                calls: Rc::new(RefCell::new(vec![])),
            });
            let output = run(Cmd::Doctor, &e).err().unwrap();
            assert!(output.contains(expected), "{output}");
            assert!(output.contains("ok plugin:"));
        }
    }
    #[cfg(unix)]
    #[test]
    fn old_version_does_not_create_division() {
        use std::os::unix::fs::PermissionsExt;
        let (t, l) = setup();
        let mut e = env(t.path(), Rc::new(RefCell::new(vec![])), false);
        let bin = t.path().join("old");
        fs::write(&bin, "#!/bin/sh\necho opencode v2.2.4\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        e.opencode = bin;
        assert!(
            run(
                Cmd::Open {
                    division: "eng".into()
                },
                &e
            )
            .err()
            .unwrap()
            .contains("2.2.5")
        );
        assert!(!l.division_dir().exists());
    }
    #[test]
    fn removal_continues_after_locked_worktree() {
        let (t, l) = setup();
        let root = t.path();
        git_ok(root, &["init"]);
        fs::write(root.join("file"), "base").unwrap();
        git_ok(root, &["add", "file"]);
        git_ok(
            root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "base",
            ],
        );
        fs::create_dir_all(l.worktrees_dir()).unwrap();
        fs::create_dir_all(l.division_dir()).unwrap();
        let locked = l.worktrees_dir().join("engineering-T-1");
        let clean = l.worktrees_dir().join("engineering-T-2");
        git_ok(
            root,
            &["worktree", "add", "-b", "locked", locked.to_str().unwrap()],
        );
        git_ok(
            root,
            &["worktree", "add", "-b", "clean", clean.to_str().unwrap()],
        );
        git_ok(root, &["worktree", "lock", locked.to_str().unwrap()]);
        let e = env(root, Rc::new(RefCell::new(vec![])), true);
        let error = run(
            Cmd::Rm {
                division: "eng".into(),
                yes: true,
            },
            &e,
        )
        .err()
        .unwrap();
        assert!(error.contains("Git worktree operation failed"));
        assert!(locked.exists());
        assert!(!clean.exists());
        assert!(l.division_dir().exists());
    }
}
