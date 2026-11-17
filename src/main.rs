use crewii::{
    cli,
    commands::{self, Env, Outcome},
    opencode, upstream,
};
use std::{
    io::{self, IsTerminal, Write},
    process::Command,
};

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let cmd = cli::parse(&args).unwrap_or_else(|e| e.exit());
    let mut bin = opencode::opencode_bin(&|name| std::env::var(name).ok());
    if bin.is_relative() && bin.components().count() > 1 {
        bin = bin
            .canonicalize()
            .map_err(|e| format!("Cannot resolve OpenCode executable: {e}"))?;
    }
    let env = Env {
        cwd: std::env::current_dir().map_err(|e| e.to_string())?,
        var: Box::new(|name| std::env::var(name).ok()),
        api: Box::new(opencode::Cli { bin: bin.clone() }),
        opencode: bin.clone(),
        upstream: (upstream::URL.into(), upstream::sha().into()),
        interactive: io::stdin().is_terminal() && io::stderr().is_terminal(),
        ask: Box::new(|prompt| {
            eprint!("{prompt}");
            let _ = io::stderr().flush();
            let mut line = String::new();
            match io::stdin().read_line(&mut line) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(line.trim().to_owned()),
            }
        }),
        confirm: Box::new(|prompt| {
            eprint!("{prompt} [y/N] ");
            let _ = io::stderr().flush();
            let mut s = String::new();
            io::stdin().read_line(&mut s).is_ok()
                && ["y", "yes"]
                    .iter()
                    .any(|v| s.trim().eq_ignore_ascii_case(v))
        }),
    };
    match commands::run(cmd, &env)? {
        Outcome::Done(text) => {
            if !text.is_empty() {
                println!("{text}");
            }
            Ok(())
        }
        Outcome::Launch { dir, env } => {
            let mut cmd = Command::new(bin);
            cmd.current_dir(dir).envs(env);
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                Err(format!(
                    "Cannot launch OpenCode: {}; check executable",
                    cmd.exec()
                ))
            }
            #[cfg(not(unix))]
            {
                let status = cmd
                    .status()
                    .map_err(|e| format!("Cannot launch OpenCode: {e}"))?;
                if status.success() {
                    Ok(())
                } else {
                    std::process::exit(status.code().unwrap_or(1))
                }
            }
        }
    }
}
