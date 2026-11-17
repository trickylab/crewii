use crate::Result;
use std::{
    fs,
    path::{Component, Path},
};

pub const URL: &str = "https://github.com/msitarzewski/agency-agents";
pub fn sha() -> &'static str {
    include_str!("../assets/agency-agents.sha").trim()
}

pub fn ensure(dir: &Path, url: &str, sha: &str) -> Result<()> {
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "Invalid commit SHA {sha:?}; provide a full 40-character hexadecimal SHA"
        ));
    }
    let clone = if !dir.exists() {
        true
    } else if dir.join(".git").exists() {
        false
    } else if dir.is_dir() {
        fs::read_dir(dir)
            .map_err(|e| {
                format!(
                    "Cannot read {}: {e}; check directory permissions",
                    dir.display()
                )
            })?
            .next()
            .is_none()
    } else {
        false
    };
    if !clone && !dir.join(".git").exists() {
        return Err(format!(
            "{} is not a git checkout; remove it and retry",
            dir.display()
        ));
    }
    if clone {
        if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Cannot create {}: {e}; check directory permissions",
                    parent.display()
                )
            })?;
        }
        let output = crate::git()
            .args(["clone", "--no-checkout", "--"])
            .arg(url)
            .arg(dir)
            .output()
            .map_err(|e| format!("Cannot run git: {e}; install Git and retry"))?;
        if !output.status.success() {
            return Err(format!(
                "Cannot clone {url}: {}; check the URL and network",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    let run = |args: &[&str]| {
        crate::git()
            .current_dir(dir)
            .args(args)
            .output()
            .map_err(|e| {
                format!(
                    "Cannot run git in {}: {e}; install Git and check the directory",
                    dir.display()
                )
            })
    };
    let head = run(&["rev-parse", "HEAD"])?;
    if head.status.success()
        && String::from_utf8_lossy(&head.stdout)
            .trim()
            .eq_ignore_ascii_case(sha)
        && !run(&["symbolic-ref", "-q", "HEAD"])?.status.success()
    {
        return Ok(());
    }
    let commit = format!("{sha}^{{commit}}");
    if !run(&["cat-file", "-e", &commit])?.status.success() {
        let fetch = run(&["fetch", "--", url])?;
        if !fetch.status.success() {
            return Err(format!(
                "Cannot fetch {url}: {}; check the URL and network",
                String::from_utf8_lossy(&fetch.stderr).trim()
            ));
        }
        if !run(&["cat-file", "-e", &commit])?.status.success() {
            let fetch = run(&["fetch", "--", url, sha])?;
            if !fetch.status.success() || !run(&["cat-file", "-e", &commit])?.status.success() {
                return Err(format!(
                    "Commit {sha} not found at {url}; check the pinned SHA, URL, and network: {}",
                    String::from_utf8_lossy(&fetch.stderr).trim()
                ));
            }
        }
    }
    let checkout = run(&["checkout", "--detach", sha])?;
    if !checkout.status.success() {
        return Err(format!(
            "Cannot checkout {sha}: {}; check the cached repository",
            String::from_utf8_lossy(&checkout.stderr).trim()
        ));
    }
    let head = run(&["rev-parse", "HEAD"])?;
    if !head.status.success()
        || !String::from_utf8_lossy(&head.stdout)
            .trim()
            .eq_ignore_ascii_case(sha)
    {
        return Err(format!(
            "HEAD does not match {sha}; check the cached repository and retry"
        ));
    }
    Ok(())
}

pub fn persona_at(dir: &Path, sha: &str, rel: &str) -> Result<String> {
    let path = Path::new(rel);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "Invalid persona path {rel:?}; use a relative path without .."
        ));
    }
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("Invalid commit SHA {sha:?}"));
    }
    let tree = crate::git()
        .current_dir(dir)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .args(["ls-tree", sha, "--", rel])
        .output()
        .map_err(|e| format!("Cannot inspect persona {rel}: {e}"))?;
    if !tree.status.success() || tree.stdout.is_empty() {
        return Err(format!(
            "Cannot inspect persona {rel}: {}; check the persona path",
            String::from_utf8_lossy(&tree.stderr).trim()
        ));
    }
    if tree.stdout.starts_with(b"120000 ") {
        return Err(format!("Persona {rel} is a symlink; choose a regular file"));
    }
    let output = crate::git()
        .current_dir(dir)
        .arg("show")
        .arg(format!("{sha}:{rel}"))
        .output()
        .map_err(|e| format!("Cannot read persona {rel}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Cannot read persona {rel}: {}; check the persona path",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text =
        String::from_utf8(output.stdout).map_err(|e| format!("Cannot read persona {rel}: {e}"))?;
    let content = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut lines = content.split_inclusive('\n');
    if lines
        .next()
        .is_some_and(|line| line.trim_end_matches(['\r', '\n']) == "---")
    {
        let mut offset = content.len() - lines.clone().map(str::len).sum::<usize>();
        for line in lines {
            offset += line.len();
            if line.trim_end_matches(['\r', '\n']) == "---" {
                return Ok(content[offset..].to_owned());
            }
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = crate::git().current_dir(dir).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn fixture() -> (TempDir, std::path::PathBuf, String) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source repo");
        fs::create_dir_all(source.join("product")).unwrap();
        git(&source, &["init"]);
        fs::write(
            source.join("product/product-manager.md"),
            "---\nname: x\n---\nBody\n",
        )
        .unwrap();
        git(&source, &["add", "."]);
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "first",
            ],
        );
        let commit = git(&source, &["rev-parse", "HEAD"]);
        (temp, source, commit)
    }
    fn commit_file(dir: &Path) -> String {
        git(dir, &["add", "."]);
        git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "persona",
            ],
        );
        git(dir, &["rev-parse", "HEAD"])
    }

    #[test]
    fn clones_and_pins() {
        let (temp, source, commit) = fixture();
        let target = temp.path().join("cached repo");
        ensure(&target, source.to_str().unwrap(), &commit).unwrap();
        assert_eq!(git(&target, &["rev-parse", "HEAD"]), commit);
        assert!(
            !crate::git()
                .current_dir(&target)
                .args(["symbolic-ref", "-q", "HEAD"])
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn same_sha_does_not_fetch() {
        let (temp, source, commit) = fixture();
        let target = temp.path().join("cached repo");
        ensure(&target, source.to_str().unwrap(), &commit).unwrap();
        fs::rename(&source, temp.path().join("moved source")).unwrap();
        ensure(&target, source.to_str().unwrap(), &commit).unwrap();
    }

    #[test]
    fn fetches_new_sha() {
        let (temp, source, first) = fixture();
        let target = temp.path().join("cached repo");
        ensure(&target, source.to_str().unwrap(), &first).unwrap();
        fs::write(source.join("new"), "new").unwrap();
        git(&source, &["add", "."]);
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "second",
            ],
        );
        let second = git(&source, &["rev-parse", "HEAD"]);
        ensure(&target, source.to_str().unwrap(), &second).unwrap();
        assert_eq!(git(&target, &["rev-parse", "HEAD"]), second);
    }

    #[test]
    fn unknown_sha_errors() {
        let (temp, source, _) = fixture();
        assert!(
            ensure(
                &temp.path().join("cache"),
                source.to_str().unwrap(),
                &"0".repeat(40)
            )
            .unwrap_err()
            .contains("not found")
        );
    }

    #[test]
    fn strips_frontmatter() {
        let (_temp, source, commit) = fixture();
        assert_eq!(
            persona_at(&source, &commit, "product/product-manager.md").unwrap(),
            "Body\n"
        );
    }
    #[test]
    fn persona_ignores_worktree_tampering() {
        let (_temp, source, commit) = fixture();
        fs::write(source.join("product/product-manager.md"), "tampered").unwrap();
        assert_eq!(
            persona_at(&source, &commit, "product/product-manager.md").unwrap(),
            "Body\n"
        );
    }
    #[test]
    fn persona_rejects_invalid_commit_and_missing_entry() {
        let (_temp, source, commit) = fixture();
        assert!(
            persona_at(&source, "invalid", "persona")
                .unwrap_err()
                .contains("Invalid commit SHA")
        );
        assert!(
            persona_at(&source, &commit, "missing")
                .unwrap_err()
                .contains("missing")
        );
        assert!(persona_at(&source, &"0".repeat(40), "missing").is_err());
    }

    #[test]
    fn no_frontmatter_kept() {
        let (_temp, source, _) = fixture();
        fs::write(source.join("plain.md"), "Plain\n").unwrap();
        let commit = commit_file(&source);
        assert_eq!(persona_at(&source, &commit, "plain.md").unwrap(), "Plain\n");
    }
    #[cfg(unix)]
    #[test]
    fn rejects_committed_symlink() {
        use std::os::unix::fs::symlink;
        let (_temp, source, _) = fixture();
        symlink("product/product-manager.md", source.join("link.md")).unwrap();
        let commit = commit_file(&source);
        assert!(
            persona_at(&source, &commit, "link.md")
                .unwrap_err()
                .contains("symlink")
        );
    }

    #[test]
    fn rejects_traversal() {
        let temp = tempfile::tempdir().unwrap();
        for path in ["../x.md", "/etc/passwd", "product/../x.md"] {
            assert!(persona_at(temp.path(), sha(), path).is_err());
        }
    }

    #[test]
    fn missing_persona() {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            persona_at(temp.path(), sha(), "missing.md")
                .unwrap_err()
                .contains("missing.md")
        );
    }

    #[test]
    fn pinned_sha_shape() {
        assert_eq!(sha().len(), 40);
        assert!(sha().bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn invalid_sha_rejected_before_clone() {
        let temp = tempfile::tempdir().unwrap();
        for value in ["short", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"] {
            assert!(
                ensure(&temp.path().join("cache"), "unused", value)
                    .unwrap_err()
                    .contains("Invalid commit SHA")
            );
        }
        assert!(!temp.path().join("cache").exists());
    }

    #[test]
    fn clone_failure_and_invalid_parent_are_actionable() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        let error = ensure(
            &temp.path().join("new/cache"),
            missing.to_str().unwrap(),
            &"0".repeat(40),
        )
        .unwrap_err();
        assert!(error.contains("check the URL and network"));
        fs::write(temp.path().join("file"), "file").unwrap();
        let error = ensure(&temp.path().join("file/cache"), "unused", &"0".repeat(40)).unwrap_err();
        assert!(error.contains("check directory permissions"));
    }

    #[test]
    fn fetch_failure_is_actionable() {
        let (temp, source, commit) = fixture();
        let target = temp.path().join("cache");
        ensure(&target, source.to_str().unwrap(), &commit).unwrap();
        let error = ensure(
            &target,
            temp.path().join("missing").to_str().unwrap(),
            &"0".repeat(40),
        )
        .unwrap_err();
        assert!(error.contains("check the URL and network"));
    }

    #[test]
    fn checkout_failure_preserves_changes() {
        let (temp, source, first) = fixture();
        let target = temp.path().join("cache");
        ensure(&target, source.to_str().unwrap(), &first).unwrap();
        let file = "product/product-manager.md";
        fs::write(source.join(file), "new version\n").unwrap();
        git(&source, &["add", "."]);
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "second",
            ],
        );
        let second = git(&source, &["rev-parse", "HEAD"]);
        fs::write(target.join(file), "local changes\n").unwrap();
        assert!(
            ensure(&target, source.to_str().unwrap(), &second)
                .unwrap_err()
                .contains("Cannot checkout")
        );
        assert_eq!(
            fs::read_to_string(target.join(file)).unwrap(),
            "local changes\n"
        );
    }

    #[test]
    fn malformed_frontmatter_kept_and_crlf_stripped() {
        let (_temp, source, _) = fixture();
        fs::write(source.join("persona"), "---\nname: x\n").unwrap();
        let commit = commit_file(&source);
        assert_eq!(
            persona_at(&source, &commit, "persona").unwrap(),
            "---\nname: x\n"
        );
        fs::write(source.join("persona"), "---\r\nname: x\r\n---\r\nBody\r\n").unwrap();
        let commit = commit_file(&source);
        assert_eq!(persona_at(&source, &commit, "persona").unwrap(), "Body\r\n");
    }

    #[test]
    fn existing_file_is_not_a_repository() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, "file").unwrap();
        let error = ensure(&file, "unused", &"0".repeat(40)).unwrap_err();
        assert!(error.contains("is not a git checkout"));
    }

    #[test]
    fn empty_nested_directory_does_not_touch_outer_repo() {
        let (_temp, outer, commit) = fixture();
        let target = outer.join("empty cache");
        fs::create_dir(&target).unwrap();
        let branch = git(&outer, &["symbolic-ref", "HEAD"]);
        ensure(&target, outer.to_str().unwrap(), &commit).unwrap();
        assert!(target.join(".git").exists());
        assert_eq!(git(&outer, &["symbolic-ref", "HEAD"]), branch);
        assert_eq!(git(&outer, &["rev-parse", "HEAD"]), commit);
    }

    #[test]
    fn nonempty_directory_is_not_checkout() {
        let (_temp, outer, commit) = fixture();
        let target = outer.join("not a checkout");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), "keep").unwrap();
        let error = ensure(&target, outer.to_str().unwrap(), &commit).unwrap_err();
        assert!(error.contains(&format!(
            "{} is not a git checkout; remove it and retry",
            target.display()
        )));
        assert_eq!(fs::read_to_string(target.join("keep")).unwrap(), "keep");
    }

    #[test]
    fn fetches_unreferenced_sha() {
        let (temp, source, first) = fixture();
        let target = temp.path().join("cache");
        ensure(&target, source.to_str().unwrap(), &first).unwrap();
        let hidden = git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit-tree",
                "HEAD^{tree}",
                "-p",
                "HEAD",
                "-m",
                "hidden",
            ],
        );
        ensure(&target, source.to_str().unwrap(), &hidden).unwrap();
        assert_eq!(git(&target, &["rev-parse", "HEAD"]), hidden);
    }

    #[test]
    fn strips_bom_before_frontmatter() {
        let (_temp, source, _) = fixture();
        fs::write(source.join("persona"), "\u{feff}---\nname: x\n---\nBody\n").unwrap();
        let commit = commit_file(&source);
        assert_eq!(persona_at(&source, &commit, "persona").unwrap(), "Body\n");
    }

    #[test]
    #[cfg(windows)]
    fn rejects_windows_root_and_prefix() {
        let temp = tempfile::tempdir().unwrap();
        for path in [r"\rooted.md", r"C:relative.md", r"C:\absolute.md"] {
            assert!(
                persona_at(temp.path(), sha(), path)
                    .unwrap_err()
                    .contains("Invalid persona path")
            );
        }
    }

    #[test]
    fn git_command_disables_interaction_and_environment_overrides() {
        let command = crate::git();
        let env: std::collections::HashMap<_, _> = command.get_envs().collect();
        for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
            assert_eq!(env.get(std::ffi::OsStr::new(key)), Some(&None));
        }
        assert_eq!(
            env.get(std::ffi::OsStr::new("GIT_TERMINAL_PROMPT")),
            Some(&Some(std::ffi::OsStr::new("0")))
        );
    }

    #[test]
    fn inherited_git_overrides_do_not_affect_checkout() {
        if std::env::var_os("UPSTREAM_ENV_TEST_CHILD").is_some() {
            let (temp, source, sha) = fixture();
            let target = temp.path().join("cache");
            ensure(&target, source.to_str().unwrap(), &sha).unwrap();
            assert_eq!(git(&target, &["rev-parse", "HEAD"]), sha);
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "upstream::tests::inherited_git_overrides_do_not_affect_checkout",
            ])
            .env("UPSTREAM_ENV_TEST_CHILD", "1")
            .env("GIT_DIR", temp.path().join("wrong-git"))
            .env("GIT_WORK_TREE", temp.path().join("wrong-tree"))
            .env("GIT_INDEX_FILE", temp.path().join("wrong-index"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
