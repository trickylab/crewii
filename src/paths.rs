//! Project root, division layout, cache and config directories.

use std::path::{Path, PathBuf};

pub struct Layout {
    pub root: PathBuf,
    pub division: String,
}
impl Layout {
    pub fn new(root: PathBuf, division: &str) -> Self {
        Self {
            root,
            division: division.to_owned(),
        }
    }
    pub fn crew_dir(&self) -> PathBuf {
        self.root.join(".opencode/crewii")
    }
    pub fn division_dir(&self) -> PathBuf {
        self.crew_dir().join(&self.division)
    }
    pub fn agents_dir(&self) -> PathBuf {
        self.division_dir()
            .join(".opencode/agents")
            .join(&self.division)
    }
    pub fn plugin_dir(&self) -> PathBuf {
        self.division_dir().join(".opencode/plugins/crewii")
    }
    pub fn state_file(&self) -> PathBuf {
        self.crew_dir()
            .join("state")
            .join(format!("{}.json", self.division))
    }
    pub fn override_file(&self) -> PathBuf {
        self.crew_dir().join(format!("{}.json", self.division))
    }
    pub fn worktrees_dir(&self) -> PathBuf {
        self.crew_dir().join("worktrees")
    }
    pub fn reports_dir(&self) -> PathBuf {
        self.crew_dir().join("reports")
    }
    pub fn tmp_dir(&self) -> PathBuf {
        self.crew_dir().join("tmp")
    }
}
pub fn find_root(cwd: &Path) -> PathBuf {
    crate::git()
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| git_path(output.stdout))
        .unwrap_or_else(|| cwd.to_path_buf())
}

fn git_path(mut bytes: Vec<u8>) -> Option<PathBuf> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes).ok().map(PathBuf::from)
    }
}
pub fn cache_dir(var: &dyn Fn(&str) -> Option<String>) -> crate::Result<PathBuf> {
    let base = var("XDG_CACHE_HOME")
        .filter(|v| Path::new(v).is_absolute())
        .map(PathBuf::from)
        .or_else(|| {
            var("HOME")
                .filter(|v| !v.is_empty())
                .map(|home| PathBuf::from(home).join(".cache"))
        })
        .ok_or_else(|| "Set XDG_CACHE_HOME or HOME to choose a cache directory".to_owned())?;
    Ok(base.join("crewii"))
}
pub fn config_dir(var: &dyn Fn(&str) -> Option<String>) -> crate::Result<PathBuf> {
    let base = var("XDG_CONFIG_HOME")
        .filter(|v| Path::new(v).is_absolute())
        .map(PathBuf::from)
        .or_else(|| {
            var("HOME")
                .filter(|v| !v.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .ok_or_else(|| "Set XDG_CONFIG_HOME or HOME to choose a config directory".to_owned())?;
    Ok(base.join("crewii"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_paths() {
        let l = Layout::new("/p q".into(), "engineering");
        assert_eq!(l.crew_dir(), Path::new("/p q/.opencode/crewii"));
        assert_eq!(
            l.division_dir(),
            Path::new("/p q/.opencode/crewii/engineering")
        );
        assert_eq!(
            l.agents_dir(),
            Path::new("/p q/.opencode/crewii/engineering/.opencode/agents/engineering")
        );
        assert_eq!(
            l.plugin_dir(),
            Path::new("/p q/.opencode/crewii/engineering/.opencode/plugins/crewii")
        );
        assert_eq!(
            l.state_file(),
            Path::new("/p q/.opencode/crewii/state/engineering.json")
        );
        assert_eq!(
            l.override_file(),
            Path::new("/p q/.opencode/crewii/engineering.json")
        );
        assert_eq!(
            l.worktrees_dir(),
            Path::new("/p q/.opencode/crewii/worktrees")
        );
        assert_eq!(l.reports_dir(), Path::new("/p q/.opencode/crewii/reports"));
        assert_eq!(l.tmp_dir(), Path::new("/p q/.opencode/crewii/tmp"));
    }

    #[test]
    fn find_root_in_git_subdir() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("a b/repo");
        let subdir = repo.join("subdir");
        std::fs::create_dir_all(&subdir).unwrap();
        assert!(
            crate::git()
                .arg("init")
                .arg(&repo)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(
            find_root(&subdir).canonicalize().unwrap(),
            repo.canonicalize().unwrap()
        );
    }

    #[test]
    fn find_root_outside_git() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(find_root(temp.path()), temp.path());
        let missing = temp.path().join("missing");
        assert_eq!(find_root(&missing), missing);
    }

    #[cfg(unix)]
    #[test]
    fn find_root_preserves_non_utf8_and_trailing_newline() {
        use std::os::unix::ffi::OsStrExt;

        let temp = tempfile::tempdir().unwrap();
        let bytes = b"/repo \xff\r\n\n";
        assert_eq!(
            git_path(bytes.to_vec()).unwrap(),
            Path::new(std::ffi::OsStr::from_bytes(b"/repo \xff\r\n"))
        );
        let repo = temp.path().join("repo \r\n");
        let subdir = repo.join("subdir");
        std::fs::create_dir_all(&subdir).unwrap();
        assert!(
            crate::git()
                .arg("init")
                .arg(&repo)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(
            find_root(&subdir).canonicalize().unwrap(),
            repo.canonicalize().unwrap()
        );
    }

    #[test]
    fn relative_xdg_paths_fall_back_to_home() {
        let vars = |name: &str| match name {
            "XDG_CACHE_HOME" | "XDG_CONFIG_HOME" => Some("relative/path".to_owned()),
            "HOME" => Some("/home".to_owned()),
            _ => None,
        };
        assert_eq!(cache_dir(&vars).unwrap(), Path::new("/home/.cache/crewii"));
        assert_eq!(
            config_dir(&vars).unwrap(),
            Path::new("/home/.config/crewii")
        );
        let no_home = |name: &str| name.starts_with("XDG_").then(|| "relative".to_owned());
        assert!(cache_dir(&no_home).is_err());
        assert!(config_dir(&no_home).is_err());
    }

    #[test]
    fn cache_and_config_dirs() {
        let xdg = |name: &str| match name {
            "XDG_CACHE_HOME" => Some("/cache dir".into()),
            "XDG_CONFIG_HOME" => Some("/config dir".into()),
            "HOME" => Some("/home".into()),
            _ => None,
        };
        assert_eq!(cache_dir(&xdg).unwrap(), Path::new("/cache dir/crewii"));
        assert_eq!(config_dir(&xdg).unwrap(), Path::new("/config dir/crewii"));
        let home = |name: &str| (name == "HOME").then(|| "/home dir".to_string());
        assert_eq!(
            cache_dir(&home).unwrap(),
            Path::new("/home dir/.cache/crewii")
        );
        assert_eq!(
            config_dir(&home).unwrap(),
            Path::new("/home dir/.config/crewii")
        );
        let none = |_: &str| None;
        assert!(cache_dir(&none).is_err());
        assert!(config_dir(&none).is_err());
    }
}
