use crate::Result;
use serde_json::Value;
#[cfg(test)]
use std::cell::RefCell;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

pub const MIN_VERSION: (u32, u32, u32) = (2, 2, 5);
pub trait Api {
    fn get(&self, dir: &Path, path: &str) -> Result<Value>;
    fn post(&self, dir: &Path, path: &str, body: &Value) -> Result<Value>;
    fn delete(&self, dir: &Path, path: &str) -> Result<()>;
    fn session_page(&self, dir: &Path, path: &str) -> Result<Value> {
        self.get(dir, path)
    }
}
pub struct Cli {
    pub bin: PathBuf,
}
impl Cli {
    fn request(&self, method: &str, dir: &Path, path: &str, body: Option<&Value>) -> Result<Value> {
        self.request_inner(method, dir, path, body, false)
    }
    fn request_inner(
        &self,
        method: &str,
        dir: &Path,
        path: &str,
        body: Option<&Value>,
        envelope: bool,
    ) -> Result<Value> {
        let mut header = String::new();
        for &byte in dir.as_os_str().as_encoded_bytes() {
            if byte < 0x20 || byte == 0x7f {
                return Err("Invalid control character in OpenCode directory".into());
            }
            if byte == b'%' || byte >= 0x80 {
                header.push_str(&format!("%{byte:02X}"));
            } else {
                header.push(byte as char);
            }
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let (temp_path, file) = loop {
            let temp_path = std::env::temp_dir().join(format!(
                "crewii-{}-{}.json",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temp_path) {
                Ok(file) => break (temp_path, file),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(format!(
                        "Cannot create temporary response file: {e}; check your temporary directory"
                    ));
                }
            }
        };
        let mut command = Command::new(&self.bin);
        command
            .args(["api", method, path, "-H"])
            .arg(format!("x-opencode-directory: {}", header));
        if let Some(body) = body {
            command.arg("-d").arg(body.to_string());
        }
        let output = command
            .stdout(Stdio::from(file))
            .stderr(Stdio::piped())
            .output();
        drop(command);
        let response = std::fs::read(&temp_path);
        let removed = std::fs::remove_file(&temp_path);
        let output = output.map_err(|e| {
            format!(
                "Cannot run {}: {e}; check the OpenCode executable",
                self.bin.display()
            )
        })?;
        if !output.status.success() {
            if method == "delete"
                && (String::from_utf8_lossy(&output.stderr).contains("404")
                    || response
                        .as_ref()
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
                        .is_some_and(|v| {
                            v.get("_tag").and_then(Value::as_str) == Some("SessionNotFoundError")
                        }))
            {
                return Ok(Value::Null);
            }
            return Err(format!(
                "OpenCode API failed: {}; stdout: {}; check the request",
                String::from_utf8_lossy(&output.stderr).trim(),
                response
                    .as_ref()
                    .map(
                        |bytes| String::from_utf8_lossy(&bytes[..bytes.len().min(200)])
                            .into_owned()
                    )
                    .unwrap_or_default()
            ));
        }
        let response = response.map_err(|e| {
            format!("Cannot read OpenCode response: {e}; check your temporary directory")
        })?;
        removed.map_err(|e| {
            format!(
                "Cannot remove temporary response {}: {e}; remove it manually",
                temp_path.display()
            )
        })?;
        if response.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        let mut value: Value = serde_json::from_slice(&response).map_err(|e| {
            format!("unexpected response from OpenCode: {e}; check the OpenCode version")
        })?;
        if method == "delete"
            && value.get("_tag").and_then(Value::as_str) == Some("SessionNotFoundError")
        {
            return Ok(Value::Null);
        }
        if envelope && value.get("data").is_some() {
            return Ok(value);
        }
        value.get_mut("data").map(Value::take).ok_or_else(|| {
            "unexpected response from OpenCode: missing data; check the OpenCode version".into()
        })
    }
}
impl Api for Cli {
    fn session_page(&self, dir: &Path, path: &str) -> Result<Value> {
        self.request_inner("get", dir, path, None, true)
    }
    fn get(&self, dir: &Path, path: &str) -> Result<Value> {
        self.request("get", dir, path, None)
    }
    fn post(&self, dir: &Path, path: &str, body: &Value) -> Result<Value> {
        self.request("post", dir, path, Some(body))
    }
    fn delete(&self, dir: &Path, path: &str) -> Result<()> {
        self.request("delete", dir, path, None).map(|_| ())
    }
}
#[cfg(test)]
pub struct Fake {
    pub calls: RefCell<Vec<(String, String, String)>>,
    responses: RefCell<Vec<(String, String, Value)>>,
}
#[cfg(test)]
impl Fake {
    pub fn new() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            responses: RefCell::new(Vec::new()),
        }
    }
    pub fn on(&self, method: &str, path_prefix: &str, data: Value) {
        self.responses
            .borrow_mut()
            .push((method.into(), path_prefix.into(), data));
    }
    fn request(&self, method: &str, dir: &Path, path: &str) -> Result<Value> {
        self.calls.borrow_mut().push((
            method.into(),
            dir.to_string_lossy().into_owned(),
            path.into(),
        ));
        self.responses
            .borrow()
            .iter()
            .rev()
            .find(|(m, prefix, _)| m == method && path.starts_with(prefix))
            .map(|(_, _, data)| data.clone())
            .ok_or_else(|| {
                format!("No fake response for {method} {path}; register one with Fake::on")
            })
    }
}
#[cfg(test)]
impl Default for Fake {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
impl Api for Fake {
    fn get(&self, dir: &Path, path: &str) -> Result<Value> {
        self.request("get", dir, path)
    }
    fn post(&self, dir: &Path, path: &str, _body: &Value) -> Result<Value> {
        self.request("post", dir, path)
    }
    fn delete(&self, dir: &Path, path: &str) -> Result<()> {
        self.request("delete", dir, path).map(|_| ())
    }
}
pub fn opencode_bin(var: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    var("CREWII_OPENCODE")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "opencode".into())
        .into()
}
pub fn version(bin: &Path) -> Result<(u32, u32, u32)> {
    let output = Command::new(bin).arg("--version").output().map_err(|e| {
        format!(
            "Cannot run {}: {e}; install OpenCode or set CREWII_OPENCODE",
            bin.display()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "OpenCode version check failed: {}; check the executable",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed = stdout.trim().strip_prefix("opencode v").and_then(|v| {
        let mut parts = v.split('-').next()?.split('.');
        let result = (
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        );
        if parts.next().is_some() {
            None
        } else {
            Some(result)
        }
    });
    parsed.ok_or_else(|| {
        format!(
            "Cannot parse OpenCode version {:?}; install OpenCode {}.{}.{} or newer",
            stdout.trim(),
            MIN_VERSION.0,
            MIN_VERSION.1,
            MIN_VERSION.2
        )
    })
}
pub fn check_version(v: (u32, u32, u32)) -> Result<()> {
    if v < MIN_VERSION {
        Err(format!(
            "OpenCode {}.{}.{} or newer is required; upgrade OpenCode",
            MIN_VERSION.0, MIN_VERSION.1, MIN_VERSION.2
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin_dir = dir.join("b i n");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let path = bin_dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn large_output_not_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let data = vec![json!("1234567890"); 20_000];
        std::fs::write(
            dir.path().join("data.json"),
            json!({"data": data}).to_string(),
        )
        .unwrap();
        let bin = script(
            dir.path(),
            "fake",
            &format!("cat '{}'", dir.path().join("data.json").display()),
        );
        let result = Cli { bin }.get(dir.path(), "/api/agent").unwrap();
        assert_eq!(result.as_array().unwrap().len(), 20_000);
    }

    #[cfg(unix)]
    #[test]
    fn header_and_body_args() {
        let dir = tempfile::tempdir().unwrap();
        let args = dir.path().join("args");
        let bin = script(
            dir.path(),
            "fake",
            &format!(
                "printf '%s\\n' \"$@\" > '{}'; printf '%s' '{{\"data\":null}}'",
                args.display()
            ),
        );
        let cli = Cli { bin };
        assert_eq!(
            cli.post(Path::new("/tmp/a b"), "/api/test", &json!({"k":1}))
                .unwrap(),
            Value::Null
        );
        assert_eq!(
            std::fs::read_to_string(&args).unwrap(),
            "api\npost\n/api/test\n-H\nx-opencode-directory: /tmp/a b\n-d\n{\"k\":1}\n"
        );
        cli.delete(Path::new("/tmp/a b"), "/api/test").unwrap();
        assert_eq!(
            std::fs::read_to_string(&args).unwrap(),
            "api\ndelete\n/api/test\n-H\nx-opencode-directory: /tmp/a b\n"
        );
        cli.get(Path::new("/tmp/%20/é dir"), "/api/test").unwrap();
        assert_eq!(
            std::fs::read_to_string(&args).unwrap(),
            "api\nget\n/api/test\n-H\nx-opencode-directory: /tmp/%2520/%C3%A9 dir\n"
        );
        cli.get(Path::new("Tài liệu/日本/%"), "/").unwrap();
        assert!(
            std::fs::read_to_string(&args)
                .unwrap()
                .contains("T%C3%A0i li%E1%BB%87u/%E6%97%A5%E6%9C%AC/%25")
        );
        assert!(cli.get(Path::new("bad\npath"), "/").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn delete_missing_session_is_success() {
        let dir = tempfile::tempdir().unwrap();
        for body in [
            "echo 'HTTP 404' >&2; exit 1",
            "echo '{\"_tag\":\"SessionNotFoundError\"}'; exit 1",
            "echo '{\"_tag\":\"SessionNotFoundError\"}'",
        ] {
            let cli = Cli {
                bin: script(dir.path(), "missing", body),
            };
            assert!(cli.delete(dir.path(), "/api/session/id").is_ok());
            assert!(cli.get(dir.path(), "/api/session/id").is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn session_page_preserves_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let cli = Cli {
            bin: script(
                dir.path(),
                "page",
                "echo '{\"data\":[],\"cursor\":{\"next\":\"token\"}}'",
            ),
        };
        assert_eq!(
            cli.session_page(dir.path(), "/api/session?limit=200")
                .unwrap()["cursor"]["next"],
            "token"
        );
        assert_eq!(
            cli.get(dir.path(), "/api/session?limit=200").unwrap(),
            json!([])
        );
    }

    #[cfg(unix)]
    #[test]
    fn nonzero_exit_errors() {
        let dir = tempfile::tempdir().unwrap();
        let bin = script(dir.path(), "fake", "echo boom >&2; exit 1");
        assert!(
            Cli { bin }
                .get(dir.path(), "/")
                .unwrap_err()
                .contains("boom")
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_data_errors() {
        let dir = tempfile::tempdir().unwrap();
        let bin = script(dir.path(), "fake", "echo '{\"x\":1}'");
        assert!(
            Cli { bin }
                .get(dir.path(), "/")
                .unwrap_err()
                .contains("unexpected response")
        );
    }

    #[cfg(unix)]
    #[test]
    fn version_parse() {
        let dir = tempfile::tempdir().unwrap();
        let bin = script(dir.path(), "good", "echo 'opencode v2.2.5'");
        assert_eq!(version(&bin).unwrap(), (2, 2, 5));
        let bin = script(dir.path(), "bad", "echo nope");
        assert!(version(&bin).is_err());
        let bin = script(dir.path(), "beta", "echo 'opencode v2.3.0-beta.1'");
        assert_eq!(version(&bin).unwrap(), (2, 3, 0));
        let bin = script(dir.path(), "dev", "echo 'opencode v2.2.5-dev'");
        assert_eq!(version(&bin).unwrap(), (2, 2, 5));
    }

    #[cfg(unix)]
    #[test]
    fn empty_response_is_null() {
        let dir = tempfile::tempdir().unwrap();
        let bin = script(dir.path(), "empty", ":");
        let cli = Cli { bin };
        assert_eq!(
            cli.post(dir.path(), "/api/session/id/command", &json!({}))
                .unwrap(),
            Value::Null
        );
        cli.delete(dir.path(), "/api/session/id").unwrap();
        let bin = script(dir.path(), "whitespace", "printf ' \\n\\t'");
        assert_eq!(Cli { bin }.get(dir.path(), "/").unwrap(), Value::Null);
    }

    #[cfg(unix)]
    #[test]
    fn failure_includes_bounded_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let bin = script(
            dir.path(),
            "error",
            "printf '%0200dEND' 0; echo boom >&2; exit 1",
        );
        let error = Cli { bin }.get(dir.path(), "/").unwrap_err();
        assert!(error.contains("boom"));
        assert!(error.contains(&"0".repeat(200)));
        assert!(!error.contains("END"));
    }

    #[cfg(unix)]
    #[test]
    fn response_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let mode = dir.path().join("mode");
        let bin = script(
            dir.path(),
            "mode",
            &format!(
                "perl -e 'printf STDERR \"%o\", (stat(STDOUT))[2] & 0777' 2> '{}'; echo '{{\"data\":null}}'",
                mode.display()
            ),
        );
        Cli { bin }.get(dir.path(), "/").unwrap();
        assert_eq!(std::fs::read_to_string(mode).unwrap().trim(), "600");
    }

    #[test]
    fn version_gate() {
        assert!(check_version((2, 2, 4)).unwrap_err().contains("2.2.5"));
        for v in [(2, 2, 5), (2, 3, 0), (3, 0, 0)] {
            assert!(check_version(v).is_ok());
        }
    }

    #[test]
    fn fake_records_and_matches() {
        let fake = Fake::new();
        fake.on("get", "/api", json!(1));
        fake.on("get", "/api/agent", json!(2));
        fake.on("get", "/api", json!(3));
        fake.on("post", "/api", json!({"ok":true}));
        fake.on("delete", "/api", Value::Null);
        let dir = Path::new("/tmp/a b");
        assert_eq!(fake.get(dir, "/api/agent").unwrap(), json!(3));
        assert_eq!(
            fake.post(dir, "/api/agent", &json!({})).unwrap(),
            json!({"ok":true})
        );
        fake.delete(dir, "/api/agent").unwrap();
        assert!(fake.get(dir, "/other").is_err());
        assert_eq!(
            *fake.calls.borrow(),
            vec![
                ("get".into(), "/tmp/a b".into(), "/api/agent".into()),
                ("post".into(), "/tmp/a b".into(), "/api/agent".into()),
                ("delete".into(), "/tmp/a b".into(), "/api/agent".into()),
                ("get".into(), "/tmp/a b".into(), "/other".into())
            ]
        );
    }

    #[test]
    fn bin_override() {
        assert_eq!(
            opencode_bin(&|name| (name == "CREWII_OPENCODE").then(|| "/tmp/my bin".into())),
            PathBuf::from("/tmp/my bin")
        );
        assert_eq!(opencode_bin(&|_| None), PathBuf::from("opencode"));
        assert_eq!(
            opencode_bin(&|_| Some(String::new())),
            PathBuf::from("opencode")
        );
    }

    #[test]
    fn fake_preserves_unicode_directory() {
        let fake = Fake::new();
        fake.on("get", "/", json!(true));
        assert_eq!(
            fake.get(Path::new("/tmp/é space"), "/").unwrap(),
            json!(true)
        );
        assert_eq!(fake.calls.borrow()[0].1, "/tmp/é space");
    }

    #[cfg(unix)]
    #[test]
    fn executable_and_response_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(
            Cli {
                bin: missing.clone()
            }
            .get(dir.path(), "/")
            .unwrap_err()
            .contains("Cannot run")
        );
        assert!(version(&missing).unwrap_err().contains("Cannot run"));
        let failed = script(dir.path(), "failed", "echo failed >&2; exit 2");
        assert!(version(&failed).unwrap_err().contains("failed"));
        let malformed = script(dir.path(), "malformed", "printf 'not json'");
        assert!(
            Cli { bin: malformed }
                .get(dir.path(), "/")
                .unwrap_err()
                .contains("unexpected response")
        );
        let extra = script(dir.path(), "extra", "echo 'opencode v2.2.5.1'");
        assert!(version(&extra).is_err());
        assert!(Fake::default().get(dir.path(), "/").is_err());
    }
}
