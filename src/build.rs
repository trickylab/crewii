//! Writes the division directory, plugin files, config, and git exclude.

use crate::{Result, paths::Layout, preset::Division, render::Rendered};
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

pub const PLUGIN_FILES: &[(&str, &str)] = &[
    ("index.js", include_str!("../assets/plugin/index.js")),
    ("tui.js", include_str!("../assets/plugin/tui.js")),
    ("core.js", include_str!("../assets/plugin/core.js")),
    ("store.js", include_str!("../assets/plugin/store.js")),
    ("tabs.js", include_str!("../assets/plugin/tabs.js")),
];
pub const EXCLUDE_LINE: &str = "/.opencode/crewii/*/";
/// Refuse when any existing path component between `root` and each dir is a symlink, so deletes stay inside root.
pub fn no_symlinks(root: &Path, dirs: &[PathBuf]) -> Result<()> {
    for dir in dirs {
        let mut path = root.to_path_buf();
        for component in dir
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .components()
        {
            path.push(component);
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(format!(
                        "{} is a symlink; remove it and retry",
                        path.display()
                    ));
                }
                Ok(_) => (),
                Err(e) if e.kind() == ErrorKind::NotFound => (),
                Err(e) => {
                    return Err(format!(
                        "Cannot inspect {}: {e}; check permissions.",
                        path.display()
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn build(layout: &Layout, d: &Division, rendered: &Rendered) -> Result<()> {
    crate::preset::validate(d)?;
    if layout.division != d.name
        || [&rendered.leads, &rendered.workers].iter().any(|agents| {
            agents.len() != d.roles.len()
                || agents
                    .iter()
                    .zip(&d.roles)
                    .any(|((id, _), role)| id != &role.id)
        })
    {
        return Err(
            "Division or rendered roles do not match; render the selected division again.".into(),
        );
    }
    no_symlinks(&layout.root, &[layout.agents_dir(), layout.plugin_dir()])?;
    for dir in [layout.agents_dir(), layout.plugin_dir()] {
        match fs::remove_dir_all(&dir) {
            Ok(()) => (),
            Err(e) if e.kind() == ErrorKind::NotFound => (),
            Err(e) => {
                return Err(format!(
                    "Cannot replace {}: {e}; check permissions.",
                    dir.display()
                ));
            }
        }
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Cannot create {}: {e}; check permissions.", dir.display()))?;
    }
    write(
        &layout.agents_dir().join("LICENSE.agency-agents"),
        include_str!("../assets/agency-agents.LICENSE").as_bytes(),
    )?;
    let worker_dir = layout.agents_dir().join("worker");
    fs::create_dir_all(&worker_dir)
        .map_err(|e| format!("Cannot create {worker_dir:?}: {e}; check permissions."))?;
    for (agents, dir) in [
        (&rendered.leads, layout.agents_dir()),
        (&rendered.workers, worker_dir),
    ] {
        for (id, content) in agents {
            write(&dir.join(format!("{id}.md")), content.as_bytes())?;
        }
    }
    for (name, content) in PLUGIN_FILES {
        write(&layout.plugin_dir().join(name), content.as_bytes())?;
    }
    write(
        &layout.plugin_dir().join("config.json"),
        config_json(d).to_string().as_bytes(),
    )?;
    Ok(())
}

fn write(path: &Path, content: &[u8]) -> Result<()> {
    fs::write(path, content)
        .map_err(|e| format!("Cannot write {}: {e}; check permissions.", path.display()))
}

fn write_exclude(path: &Path, content: &[u8]) -> Result<()> {
    let mut temp_name = path.as_os_str().to_os_string();
    temp_name.push(format!(".crewii-{}.tmp", std::process::id()));
    let temp = PathBuf::from(temp_name);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| {
            format!(
                "Cannot create {}: {e}; remove any stale temporary file and check permissions.",
                temp.display()
            )
        })?;
    let result = file.write_all(content).and_then(|()| {
        drop(file);
        fs::rename(&temp, path)
    });
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|e| format!("Cannot replace {}: {e}; check permissions.", path.display()))
}

pub fn config_json(d: &Division) -> serde_json::Value {
    serde_json::json!({
        "division": d.name,
        "agentPrefix": format!("{}/", d.name),
        "roles": d.roles.iter().map(|role| serde_json::json!({"id": role.id, "description": role.description})).collect::<Vec<_>>(),
        "limits": d.limits,
    })
}

fn exclude_path(root: &Path) -> Result<Option<PathBuf>> {
    let output = crate::git()
        .current_dir(root)
        .args(["rev-parse", "--git-path", "info/exclude"])
        .output();
    let output = match output {
        Ok(output) => output,
        Err(e) if e.kind() == ErrorKind::NotFound && root.is_dir() => return Ok(None),
        Err(e) => {
            return Err(format!(
                "Cannot run git in {}: {e}; install git and check the directory.",
                root.display()
            ));
        }
    };
    if !output.status.success() {
        return Ok(None);
    }
    let mut bytes = output.stdout;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(bytes))
    };
    #[cfg(not(unix))]
    let path = PathBuf::from(
        String::from_utf8(bytes)
            .map_err(|e| format!("Invalid git exclude path: {e}; check the repository path."))?,
    );
    Ok(Some(if path.is_absolute() {
        path
    } else {
        root.join(path)
    }))
}

fn read_exclude(path: &Path) -> Result<Vec<u8>> {
    match fs::read(path) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!(
            "Cannot read {}: {e}; check permissions.",
            path.display()
        )),
    }
}

fn is_exclude(line: &[u8]) -> bool {
    line.strip_suffix(b"\n")
        .unwrap_or(line)
        .strip_suffix(b"\r")
        .unwrap_or(line.strip_suffix(b"\n").unwrap_or(line))
        == EXCLUDE_LINE.as_bytes()
}

pub fn ensure_exclude(root: &Path) -> Result<bool> {
    let Some(path) = exclude_path(root)? else {
        return Ok(false);
    };
    let mut content = read_exclude(&path)?;
    if content.split_inclusive(|b| *b == b'\n').any(is_exclude) {
        return Ok(false);
    }
    if !content.is_empty() && !content.ends_with(b"\n") {
        content.push(b'\n');
    }
    content.extend_from_slice(EXCLUDE_LINE.as_bytes());
    content.push(b'\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create {parent:?}: {e}; check permissions."))?;
    }
    write_exclude(&path, &content)?;
    Ok(true)
}

pub fn remove_exclude(root: &Path) -> Result<()> {
    // ponytail: linked worktrees share exclude; upgrade: scan git worktree list roots.
    let crew = root.join(".opencode/crewii");
    match fs::read_dir(&crew) {
        Ok(entries) => {
            for entry in entries {
                let entry =
                    entry.map_err(|e| format!("Cannot read {crew:?}: {e}; check permissions."))?;
                if entry
                    .file_type()
                    .map_err(|e| {
                        format!("Cannot inspect {:?}: {e}; check permissions.", entry.path())
                    })?
                    .is_dir()
                {
                    let path = entry.path();
                    if let Some(item) = fs::read_dir(&path)
                        .map_err(|e| format!("Cannot read {path:?}: {e}; check permissions."))?
                        .next()
                    {
                        item.map_err(|e| format!("Cannot read {path:?}: {e}; check permissions."))?;
                        return Ok(());
                    }
                }
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => (),
        Err(e) => {
            return Err(format!(
                "Cannot read {}: {e}; check permissions.",
                crew.display()
            ));
        }
    }
    let Some(path) = exclude_path(root)? else {
        return Ok(());
    };
    let content = read_exclude(&path)?;
    let filtered: Vec<u8> = content
        .split_inclusive(|b| *b == b'\n')
        .filter(|line| !is_exclude(line))
        .flatten()
        .copied()
        .collect();
    if filtered != content {
        write_exclude(&path, &filtered)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo space");
        fs::create_dir(&root).unwrap();
        assert!(
            crate::git()
                .arg("init")
                .arg(&root)
                .output()
                .unwrap()
                .status
                .success()
        );
        (temp, root)
    }
    fn sample() -> (Division, Rendered) {
        let d = crate::preset::load("engineering", None).unwrap();
        let r = crate::render::render(&d, None, None, &|_| Ok("persona".into())).unwrap();
        (d, r)
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_output_ancestors() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir_all(outside.join("crewii/engineering/.opencode/agents/engineering"))
            .unwrap();
        let victim = outside.join("crewii/engineering/.opencode/agents/engineering/victim");
        fs::write(&victim, "keep").unwrap();
        symlink(&outside, root.join(".opencode")).unwrap();
        let (d, r) = sample();
        assert!(
            build(&Layout::new(root, "engineering"), &d, &r)
                .unwrap_err()
                .contains("is a symlink")
        );
        assert_eq!(fs::read_to_string(victim).unwrap(), "keep");
    }
    #[test]
    fn stale_exclude_temp_from_another_process_does_not_block() {
        let (_temp, root) = repo();
        fs::write(
            root.join(format!(
                ".git/info/exclude.crewii-{}.tmp",
                u64::from(std::process::id()) + 1
            )),
            "stale",
        )
        .unwrap();
        assert!(ensure_exclude(&root).unwrap());
    }
    #[test]
    fn writes_agency_license() {
        let (_temp, root) = repo();
        let layout = Layout::new(root, "engineering");
        let (d, r) = sample();
        build(&layout, &d, &r).unwrap();
        assert_eq!(
            fs::read_to_string(layout.agents_dir().join("LICENSE.agency-agents")).unwrap(),
            include_str!("../assets/agency-agents.LICENSE")
        );
    }
    #[test]
    fn writes_agents_plugin_config() {
        let (_temp, root) = repo();
        let layout = Layout::new(root, "engineering");
        let (d, r) = sample();
        build(&layout, &d, &r).unwrap();
        let entries: Vec<_> = fs::read_dir(layout.agents_dir())
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        assert_eq!(entries.len(), 9);
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.file_type().unwrap().is_file())
                .count(),
            8
        );
        let workers: Vec<_> = fs::read_dir(layout.agents_dir().join("worker"))
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        assert_eq!(workers.len(), 7);
        assert!(workers.iter().all(|e| e.file_type().unwrap().is_file()));
        for (id, text) in &r.leads {
            assert_eq!(
                fs::read_to_string(layout.agents_dir().join(format!("{id}.md"))).unwrap(),
                *text
            );
        }
        for (id, text) in &r.workers {
            assert_eq!(
                fs::read_to_string(layout.agents_dir().join("worker").join(format!("{id}.md")))
                    .unwrap(),
                *text
            );
        }
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(layout.plugin_dir().join("config.json")).unwrap())
                .unwrap();
        assert_eq!(config, config_json(&d));
        assert_eq!(config["division"], "engineering");
        assert_eq!(config["agentPrefix"], "engineering/");
        assert_eq!(config["roles"].as_array().unwrap().len(), 7);
        for (role, expected) in config["roles"].as_array().unwrap().iter().zip(&d.roles) {
            assert_eq!(
                role,
                &serde_json::json!({"id": expected.id, "description": expected.description})
            );
        }
        assert_eq!(
            config["limits"],
            serde_json::json!({"instancesPerRole":3,"workersPerTicket":5,"ticketsPerBa":3,"workerSteps":60,"staleMinutes":15})
        );
        assert_eq!(fs::read_dir(layout.plugin_dir()).unwrap().count(), 6);
        for (name, text) in PLUGIN_FILES {
            assert_eq!(
                fs::read_to_string(layout.plugin_dir().join(name)).unwrap(),
                *text
            );
        }
    }
    #[test]
    fn stale_role_removed_and_other_data_preserved() {
        let (_temp, root) = repo();
        let layout = Layout::new(root, "engineering");
        fs::create_dir_all(layout.agents_dir()).unwrap();
        fs::write(layout.agents_dir().join("old.md"), "old").unwrap();
        fs::create_dir_all(layout.plugin_dir()).unwrap();
        fs::write(layout.plugin_dir().join("old.js"), "old").unwrap();
        fs::create_dir_all(layout.state_file().parent().unwrap()).unwrap();
        fs::create_dir_all(layout.worktrees_dir()).unwrap();
        for path in [
            layout.override_file(),
            layout.state_file(),
            layout.worktrees_dir().join("keep"),
        ] {
            fs::write(path, "keep").unwrap();
        }
        let (d, r) = sample();
        build(&layout, &d, &r).unwrap();
        assert!(!layout.agents_dir().join("old.md").exists());
        assert!(!layout.plugin_dir().join("old.js").exists());
        for path in [
            layout.override_file(),
            layout.state_file(),
            layout.worktrees_dir().join("keep"),
        ] {
            assert_eq!(fs::read_to_string(path).unwrap(), "keep");
        }
    }
    #[test]
    fn exclude_added_once_and_removed_when_empty() {
        let (_temp, root) = repo();
        let path = root.join(".git/info/exclude");
        fs::write(&path, "existing").unwrap();
        assert!(ensure_exclude(&root).unwrap());
        assert!(!ensure_exclude(&root).unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("existing\n{EXCLUDE_LINE}\n")
        );
        let layout = Layout::new(root.clone(), "engineering");
        fs::create_dir_all(layout.division_dir()).unwrap();
        fs::write(layout.division_dir().join("keep"), "data").unwrap();
        remove_exclude(&root).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains(EXCLUDE_LINE));
        fs::remove_dir_all(layout.division_dir()).unwrap();
        fs::create_dir_all(layout.worktrees_dir()).unwrap();
        fs::create_dir_all(layout.state_file().parent().unwrap()).unwrap();
        fs::write(layout.override_file(), "keep").unwrap();
        remove_exclude(&root).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "existing\n");
    }
    #[test]
    fn exclude_skipped_outside_git() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!ensure_exclude(temp.path()).unwrap());
        remove_exclude(temp.path()).unwrap();
        assert!(!temp.path().join(".git").exists());
    }

    #[test]
    fn exclude_kept_for_nonempty_crew_directories() {
        for name in ["worktrees", "state", "engineering"] {
            let (_temp, root) = repo();
            let exclude = root.join(".git/info/exclude");
            let dir = root.join(".opencode/crewii").join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("retained"), "data").unwrap();
            ensure_exclude(&root).unwrap();
            remove_exclude(&root).unwrap();
            assert!(
                fs::read_to_string(&exclude)
                    .unwrap()
                    .lines()
                    .any(|line| line == EXCLUDE_LINE),
                "{name}"
            );
            fs::remove_file(dir.join("retained")).unwrap();
            remove_exclude(&root).unwrap();
            assert!(
                !fs::read_to_string(&exclude)
                    .unwrap()
                    .lines()
                    .any(|line| line == EXCLUDE_LINE),
                "{name}"
            );
        }
    }

    #[test]
    fn rejects_mismatched_and_invalid_roles_without_removing_files() {
        let (_temp, root) = repo();
        let layout = Layout::new(root.clone(), "engineering");
        fs::create_dir_all(layout.agents_dir()).unwrap();
        fs::write(layout.agents_dir().join("keep"), "keep").unwrap();
        let (mut d, mut r) = sample();
        r.leads[0].0 = "../bad".into();
        assert!(build(&layout, &d, &r).is_err());
        assert!(build(&Layout::new(root, "other"), &d, &r).is_err());
        d.roles[0].id = "../bad".into();
        assert!(build(&layout, &d, &r).is_err());
        assert!(layout.agents_dir().join("keep").exists());
    }

    #[test]
    fn reports_filesystem_errors() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::write(&root, "file").unwrap();
        let layout = Layout::new(root.clone(), "engineering");
        let (d, r) = sample();
        assert!(build(&layout, &d, &r).is_err());
        assert!(write(&root.join("file"), b"x").is_err());
        assert!(read_exclude(&root).is_ok());
        assert!(read_exclude(temp.path()).is_err());
        assert!(exclude_path(&root).is_err());
        assert!(remove_exclude(&root).is_err());
        let (_temp, root) = repo();
        let layout = Layout::new(root.clone(), "engineering");
        fs::create_dir_all(layout.agents_dir().parent().unwrap()).unwrap();
        fs::write(layout.agents_dir(), "file").unwrap();
        assert!(build(&layout, &d, &r).is_err());
        let crew = root.join(".opencode/crewii");
        fs::remove_dir_all(&crew).unwrap();
        fs::write(&crew, "file").unwrap();
        assert!(remove_exclude(&root).is_err());
    }

    #[test]
    fn separate_git_directory_and_exclude_line_endings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo space");
        let git_dir = temp.path().join("git dir");
        assert!(
            crate::git()
                .arg("init")
                .arg("--separate-git-dir")
                .arg(&git_dir)
                .arg(&root)
                .output()
                .unwrap()
                .status
                .success()
        );
        let path = git_dir.join("info/exclude");
        fs::remove_file(&path).unwrap();
        fs::remove_dir(git_dir.join("info")).unwrap();
        assert!(ensure_exclude(&root).unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("{EXCLUDE_LINE}\n")
        );
        fs::write(&path, format!("first\r\n{EXCLUDE_LINE}\r\nlast")).unwrap();
        assert!(!ensure_exclude(&root).unwrap());
        remove_exclude(&root).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\r\nlast");
        remove_exclude(&root).unwrap();
    }

    #[test]
    fn exclude_read_errors_are_reported() {
        let (_temp, root) = repo();
        let path = root.join(".git/info/exclude");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(ensure_exclude(&root).is_err());
        assert!(remove_exclude(&root).is_err());
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".opencode/crewii/state")).unwrap();
        remove_exclude(temp.path()).unwrap();
    }

    #[test]
    fn linked_worktree_uses_shared_exclude_once() {
        let (_temp, root) = repo();
        let git = |args: &[&str]| {
            let output = crate::git().current_dir(&root).args(args).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .unwrap()
                .trim_end()
                .to_owned()
        };
        let tree = git(&["hash-object", "-t", "tree", "-w", "--stdin"]);
        let commit = git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit-tree",
            &tree,
            "-m",
            "initial",
        ]);
        let worktree = root.parent().unwrap().join("linked worktree");
        git(&[
            "worktree",
            "add",
            "--detach",
            worktree.to_str().unwrap(),
            &commit,
        ]);
        let exclude = root.join(git(&["rev-parse", "--git-path", "info/exclude"]));
        assert!(ensure_exclude(&worktree).unwrap());
        assert!(!ensure_exclude(&worktree).unwrap());
        let content = fs::read_to_string(&exclude).unwrap();
        assert_eq!(
            content.lines().filter(|line| *line == EXCLUDE_LINE).count(),
            1
        );
        remove_exclude(&worktree).unwrap();
        assert!(
            !fs::read_to_string(exclude)
                .unwrap()
                .lines()
                .any(|line| line == EXCLUDE_LINE)
        );
    }

    #[test]
    fn plugin_file_list_matches_assets() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/plugin");
        let mut assets: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "js"))
            .map(|p| p.file_name().unwrap().to_owned())
            .collect();
        let mut embedded: Vec<_> = PLUGIN_FILES
            .iter()
            .map(|(name, _)| std::ffi::OsString::from(name))
            .collect();
        assets.sort();
        embedded.sort();
        assert_eq!(assets, embedded);
    }

    #[test]
    fn missing_git_is_skipped() {
        if std::env::var_os("CREWII_TEST_NO_GIT").is_some() {
            let temp = tempfile::tempdir().unwrap();
            assert!(!ensure_exclude(temp.path()).unwrap());
            remove_exclude(temp.path()).unwrap();
        } else {
            let empty = tempfile::tempdir().unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "build::tests::missing_git_is_skipped"])
                .env("CREWII_TEST_NO_GIT", "1")
                .env("PATH", empty.path())
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    #[test]
    fn exclude_temp_write_failure_preserves_original() {
        let (_temp, root) = repo();
        let path = root.join(".git/info/exclude");
        let temp_path = root.join(format!(
            ".git/info/exclude.crewii-{}.tmp",
            std::process::id()
        ));
        fs::write(&path, "keep\n").unwrap();
        fs::create_dir(&temp_path).unwrap();
        assert!(ensure_exclude(&root).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep\n");
        fs::write(&path, format!("keep\n{EXCLUDE_LINE}\n")).unwrap();
        assert!(remove_exclude(&root).is_err());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("keep\n{EXCLUDE_LINE}\n")
        );
    }
}
