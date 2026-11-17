pub mod build;
pub mod check;
pub mod cli;
pub mod commands;
pub mod models;
pub mod opencode;
pub mod paths;
pub mod perm;
pub mod preset;
pub mod render;
pub mod state;
pub mod upstream;

pub type Result<T> = std::result::Result<T, String>;

/// `git` with repository env scrubbed, no prompts, and C locale (stable stderr text).
pub fn git() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    command
}
