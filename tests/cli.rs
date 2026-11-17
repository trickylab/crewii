use std::process::Command;

#[test]
fn binary_help_unknown_and_init() {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_crewii"))
            .current_dir(dir.path())
            .env("HOME", dir.path())
            .env("XDG_CONFIG_HOME", dir.path())
            .env("XDG_CACHE_HOME", dir.path())
            .env("CREWII_OPENCODE", dir.path().join("missing"))
            .args(args)
            .output()
            .unwrap()
    };
    let help = run(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Usage"));
    assert!(String::from_utf8_lossy(&help.stdout).contains("CREWII_OPENCODE"));
    assert_eq!(run(&["status", "--bad"]).status.code(), Some(2));
    assert!(run(&["--version"]).status.success());
    let unknown = run(&["unknown"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("engineering"));
    assert!(run(&["init", "eng"]).status.success());
    assert!(
        dir.path()
            .join(".opencode/crewii/engineering.json")
            .exists()
    );
    assert!(!run(&["init", "eng"]).status.success());
}

#[cfg(unix)]
#[test]
fn executable_override_path_and_non_utf8_args() {
    use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("fake-opencode");
    std::fs::write(&bin, "#!/bin/sh\necho 'opencode v2.2.5'\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run = |value: &str| {
        Command::new(env!("CARGO_BIN_EXE_crewii"))
            .current_dir(dir.path())
            .env("PATH", dir.path())
            .env("CREWII_OPENCODE", value)
            .args(["doctor"])
            .output()
            .unwrap()
    };
    assert!(
        String::from_utf8_lossy(&run("fake-opencode").stdout).contains("ok opencode: 2.2.5")
            || String::from_utf8_lossy(&run("fake-opencode").stderr).contains("ok opencode: 2.2.5")
    );
    assert!(
        !String::from_utf8_lossy(&run("").stderr).contains("Cannot resolve OpenCode executable")
    );
    let output = Command::new(env!("CARGO_BIN_EXE_crewii"))
        .current_dir(dir.path())
        .arg(std::ffi::OsString::from_vec(vec![0xff]))
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
}
