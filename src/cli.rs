use clap::{Parser, Subcommand};

#[derive(Debug, PartialEq, Subcommand)]
pub enum Cmd {
    #[command(about = "Choose primary and subagent models")]
    Models {
        #[arg(long, help = "Lead model, provider/model[#variant]")]
        primary: Option<String>,
        #[arg(long, help = "Worker model, provider/model[#variant]")]
        subagent: Option<String>,
        #[arg(long, help = "Save as the global default instead of this directory")]
        global: bool,
    },
    #[command(about = "List crews in this project")]
    List,
    #[command(about = "Build and open a crew")]
    Open {
        #[arg(help = "Division name or alias")]
        division: String,
    },
    #[command(about = "Send a request to a crew")]
    Ask {
        #[arg(help = "Division name or alias")]
        division: String,
        #[arg(help = "Request text, optionally unquoted", trailing_var_arg = true, num_args = 1.., allow_hyphen_values = true)]
        text: Vec<String>,
    },
    #[command(about = "Show crew status")]
    Status {
        #[arg(help = "Division name or alias; omit for all")]
        division: Option<String>,
        #[arg(long, help = "Remove confirmed clean worktrees for closed tickets")]
        prune: bool,
    },
    #[command(about = "Remove a crew and its sessions")]
    Rm {
        #[arg(help = "Division name or alias")]
        division: String,
        #[arg(long, help = "Skip the confirmation only")]
        yes: bool,
    },
    #[command(about = "Create a project override")]
    Init {
        #[arg(help = "Division name or alias")]
        division: String,
    },
    #[command(about = "Check requirements and configuration")]
    Doctor,
    #[command(about = "Check crew isolation and permissions")]
    Check {
        #[arg(help = "Division name or alias; omit for all")]
        division: Option<String>,
    },
}

#[derive(Parser)]
#[command(
    name = "crewii",
    version,
    about = "Role-based crews for OpenCode",
    long_about = format!("Role-based crews for OpenCode.\n\ncrewii <division> opens a crew (alias eng); no args lists divisions.\nDivisions: engineering. Ask text may be unquoted. Requires OpenCode >= {}.{}.{}.", crate::opencode::MIN_VERSION.0, crate::opencode::MIN_VERSION.1, crate::opencode::MIN_VERSION.2),
    after_help = "Files: ~/.config/crewii/config.json (global models); <root>/.opencode/crewii/state/models.json (directory models); <root>/.opencode/crewii/<division>.json (override).\nEnvironment: CREWII_OPENCODE, XDG_CONFIG_HOME, XDG_CACHE_HOME.\nrm --yes only skips the confirmation."
)]
struct Args {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

pub fn parse(args: &[String]) -> std::result::Result<Cmd, clap::Error> {
    let mut args = args.to_vec();
    if args.is_empty() {
        args.push("crewii".into());
    }
    if args
        .get(1)
        .is_some_and(|s| !s.starts_with('-') && !crate::preset::RESERVED.contains(&s.as_str()))
    {
        args.insert(1, "open".into());
    }
    Args::try_parse_from(args).map(|a| a.cmd.unwrap_or(Cmd::List))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn models_command_is_not_a_division() {
        assert_eq!(
            parse(&[
                "crewii".into(),
                "models".into(),
                "--primary".into(),
                "a".into(),
                "--global".into()
            ])
            .unwrap(),
            Cmd::Models {
                primary: Some("a".into()),
                subagent: None,
                global: true
            }
        );
    }
    #[test]
    fn variants() {
        let p =
            |args: &[&str]| parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        assert_eq!(p(&["crewii"]), Cmd::List);
        assert_eq!(
            p(&["crewii", "eng"]),
            Cmd::Open {
                division: "eng".into()
            }
        );
        assert_eq!(
            p(&["crewii", "ask", "eng", "hi"]),
            Cmd::Ask {
                division: "eng".into(),
                text: vec!["hi".into()]
            }
        );
        assert_eq!(
            p(&["crewii", "status", "--prune"]),
            Cmd::Status {
                division: None,
                prune: true
            }
        );
        assert_eq!(
            p(&["crewii", "rm", "eng", "--yes"]),
            Cmd::Rm {
                division: "eng".into(),
                yes: true
            }
        );
        assert_eq!(p(&["crewii", "check"]), Cmd::Check { division: None });
        assert_eq!(
            p(&["crewii", "ask", "eng", "hello", "world"]),
            Cmd::Ask {
                division: "eng".into(),
                text: vec!["hello".into(), "world".into()]
            }
        );
        assert_eq!(
            p(&["crewii", "ask", "eng", "fix", "the", "bug"]),
            Cmd::Ask {
                division: "eng".into(),
                text: vec!["fix".into(), "the".into(), "bug".into()]
            }
        );
        assert_eq!(
            p(&["crewii", "ask", "eng", "--", "-x", "fix"]),
            Cmd::Ask {
                division: "eng".into(),
                text: vec!["-x".into(), "fix".into()]
            }
        );
        assert!(parse(&["crewii".into(), "--bad".into()]).is_err());
    }
}
