//! Division presets, user overrides, and validation.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{
    Result,
    perm::{Access, Rule},
};

pub const RESERVED: &[&str] = &[
    "list", "open", "help", "ask", "status", "rm", "init", "doctor", "check", "models",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub instances_per_role: u32,
    pub workers_per_ticket: u32,
    pub tickets_per_ba: u32,
    pub worker_steps: u32,
    #[serde(default = "default_stale_minutes")]
    pub stale_minutes: u32,
}

fn default_stale_minutes() -> u32 {
    15
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Role {
    pub id: String,
    pub persona: String,
    pub description: String,
    pub boundary: String,
    pub access: Access,
    #[serde(default)]
    pub permissions: Vec<Rule>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Floor {
    #[serde(default)]
    pub remove: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Division {
    pub name: String,
    pub aliases: Vec<String>,
    pub limits: Limits,
    pub roles: Vec<Role>,
    #[serde(default)]
    pub floor: Floor,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Override {
    limits: Option<LimitOverride>,
    floor: Option<Floor>,
    roles: Option<Vec<RoleOverride>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LimitOverride {
    instances_per_role: Option<u32>,
    workers_per_ticket: Option<u32>,
    tickets_per_ba: Option<u32>,
    worker_steps: Option<u32>,
    stale_minutes: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleOverride {
    id: String,
    persona: Option<String>,
    boundary: Option<String>,
    description: Option<String>,
    permissions: Option<Vec<Rule>>,
}

pub fn builtin_names() -> Vec<&'static str> {
    vec!["engineering"]
}

pub fn resolve_name(name: &str) -> Option<&'static str> {
    let division: Division =
        serde_json::from_str(include_str!("../assets/divisions/engineering.json")).ok()?;
    (division.name == name || division.aliases.iter().any(|alias| alias == name))
        .then_some("engineering")
}

pub fn load(name: &str, override_json: Option<&str>) -> Result<Division> {
    let name = resolve_name(name)
        .ok_or_else(|| format!("Unknown division '{name}'; choose a built-in division."))?;
    let mut division: Division = serde_json::from_str(include_str!(
        "../assets/divisions/engineering.json"
    ))
    .map_err(|error| format!("Invalid built-in {name}.json: {error}; repair the preset."))?;
    if let Some(json) = override_json {
        let overrides: Override = serde_json::from_str(json)
            .map_err(|error| format!("Invalid {name}.json override: {error}; correct the file."))?;
        if let Some(limits) = overrides.limits {
            if let Some(value) = limits.instances_per_role {
                division.limits.instances_per_role = value;
            }
            if let Some(value) = limits.workers_per_ticket {
                division.limits.workers_per_ticket = value;
            }
            if let Some(value) = limits.tickets_per_ba {
                division.limits.tickets_per_ba = value;
            }
            if let Some(value) = limits.worker_steps {
                division.limits.worker_steps = value;
            }
            if let Some(value) = limits.stale_minutes {
                division.limits.stale_minutes = value;
            }
        }
        if let Some(floor) = overrides.floor {
            division.floor = floor;
        }
        for changes in overrides.roles.unwrap_or_default() {
            let role = division
                .roles
                .iter_mut()
                .find(|role| role.id == changes.id)
                .ok_or_else(|| {
                    format!(
                        "Unknown role '{}' in {name}.json; use an existing role id.",
                        changes.id
                    )
                })?;
            if let Some(value) = changes.persona {
                role.persona = value;
            }
            if let Some(value) = changes.boundary {
                role.boundary = value;
            }
            if let Some(value) = changes.description {
                role.description = value;
            }
            if let Some(value) = changes.permissions {
                role.permissions.extend(value);
            }
        }
    }
    let floor = crate::perm::floor(&[]);
    for resource in &division.floor.remove {
        if !floor.iter().any(|rule| rule.resource == *resource) {
            return Err(format!("floor.remove: unknown floor rule '{resource}'"));
        }
    }
    validate(&division)?;
    Ok(division)
}

pub fn validate(division: &Division) -> Result<()> {
    for (name, value) in [
        ("instancesPerRole", division.limits.instances_per_role),
        ("workersPerTicket", division.limits.workers_per_ticket),
        ("ticketsPerBa", division.limits.tickets_per_ba),
        ("workerSteps", division.limits.worker_steps),
    ] {
        if value == 0 {
            return Err(format!(
                "limits.{name} must be at least 1; set a positive number"
            ));
        }
    }
    if RESERVED.contains(&division.name.as_str()) {
        return Err(format!(
            "Reserved division name '{}'; choose another name.",
            division.name
        ));
    }
    let mut ids = HashSet::new();
    for role in &division.roles {
        let mut chars = role.id.bytes();
        if !chars.next().is_some_and(|c| c.is_ascii_lowercase())
            || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            || role.id.rsplit_once('-').is_some_and(|(_, suffix)| {
                !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit())
            })
        {
            return Err(format!(
                "Invalid role id '{}'; use lowercase letters, digits, and hyphens, starting with a letter and without a numeric suffix after a hyphen.",
                role.id
            ));
        }
        if !ids.insert(&role.id) {
            return Err(format!("Duplicate role id '{}'; use unique ids.", role.id));
        }
    }
    if !ids.contains(&"ba".to_string()) {
        return Err("Missing ba role; add exactly one ba role.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_minutes_default_overrides_and_plugin_config() {
        for (overrides, expected) in [
            (None, 15),
            (Some(r#"{"limits":{"staleMinutes":9}}"#), 9),
            (Some(r#"{"limits":{"staleMinutes":0}}"#), 0),
        ] {
            let division = load("eng", overrides).unwrap();
            assert_eq!(
                crate::build::config_json(&division)["limits"]["staleMinutes"],
                expected
            );
        }
        assert!(load("eng", Some(r#"{"limits":{"staleMinutes":-1}}"#)).is_err());
    }

    #[test]
    fn worker_steps_default_override_and_validation() {
        let d = load("eng", None).unwrap();
        assert_eq!(serde_json::to_value(&d.limits).unwrap()["workerSteps"], 60);
        let d = load("eng", Some(r#"{"limits":{"workerSteps":17}}"#)).unwrap();
        assert_eq!(serde_json::to_value(&d.limits).unwrap()["workerSteps"], 17);
        assert!(
            load("eng", Some(r#"{"limits":{"workerSteps":0}}"#))
                .unwrap_err()
                .contains("limits.workerSteps must be at least 1")
        );
    }

    #[test]
    fn removed_override_fields_are_rejected() {
        for json in [
            r#"{"models":{}}"#,
            r#"{"roles":[{"id":"ba","tier":"fast"}]}"#,
        ] {
            assert!(load("engineering", Some(json)).is_err(), "{json}");
        }
    }

    #[test]
    fn engineering_loads() {
        let d = load("engineering", None).unwrap();
        assert_eq!(builtin_names(), vec!["engineering"]);
        assert_eq!(
            d.roles.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            [
                "ba",
                "architect",
                "backend",
                "frontend",
                "reviewer",
                "qa",
                "security"
            ]
        );
        assert_eq!(d.aliases, ["eng"]);
        assert_eq!(
            (
                d.limits.instances_per_role,
                d.limits.workers_per_ticket,
                d.limits.tickets_per_ba
            ),
            (3, 5, 3)
        );
    }

    #[test]
    fn alias() {
        assert_eq!(resolve_name("eng"), Some("engineering"));
        assert_eq!(resolve_name("nope"), None);
        assert!(load("nope", None).unwrap_err().contains("Unknown division"));
        assert_eq!(load("eng", None).unwrap().name, "engineering");
        for alias in load("engineering", None).unwrap().aliases {
            assert_eq!(resolve_name(&alias), Some("engineering"));
        }
        for name in [
            "list", "open", "help", "ask", "status", "rm", "init", "doctor", "check",
        ] {
            let mut division = load("eng", None).unwrap();
            division.name = name.into();
            assert!(validate(&division).is_err(), "{name}");
        }
    }

    #[test]
    fn override_replaces_and_appends() {
        let json = r#"{"floor":{"remove":["gh api*"]},"roles":[{"id":"ba","persona":"a.md","boundary":"new","description":"new desc","permissions":[{"action":"read","resource":"x","effect":"deny"}]},{"id":"ba","permissions":[{"action":"read","resource":"y","effect":"allow"}]}]}"#;
        let d = load("engineering", Some(json)).unwrap();
        let role = &d.roles[0];
        assert_eq!(
            (&*role.persona, &*role.boundary, &*role.description),
            ("a.md", "new", "new desc")
        );
        assert_eq!(
            role.permissions
                .iter()
                .map(|r| r.resource.as_str())
                .collect::<Vec<_>>(),
            ["x", "y"]
        );
        assert_eq!(d.floor.remove, ["gh api*"]);
    }

    #[test]
    fn override_unknown_role() {
        assert!(
            load("engineering", Some(r#"{"roles":[{"id":"unknown"}]}"#))
                .unwrap_err()
                .to_lowercase()
                .contains("unknown role")
        );
    }

    #[test]
    fn override_typo_field() {
        for json in [
            r#"{"rols":[]}"#,
            r#"{"models":{"typo":1}}"#,
            r#"{"limits":{"typo":1}}"#,
            r#"{"floor":{"typo":1}}"#,
            r#"{"roles":[{"id":"ba","access":"writer"}]}"#,
            "{",
        ] {
            assert!(
                load("engineering", Some(json))
                    .unwrap_err()
                    .contains("engineering.json")
            );
        }
    }

    #[test]
    fn override_limits() {
        let d = load("engineering", Some(r#"{"limits":{"instancesPerRole":2}}"#)).unwrap();
        assert_eq!(
            (
                d.limits.instances_per_role,
                d.limits.workers_per_ticket,
                d.limits.tickets_per_ba
            ),
            (2, 5, 3)
        );
        let d = load(
            "eng",
            Some(r#"{"limits":{"workersPerTicket":1,"ticketsPerBa":2}}"#),
        )
        .unwrap();
        assert_eq!(
            (d.limits.workers_per_ticket, d.limits.tickets_per_ba),
            (1, 2)
        );
    }

    #[test]
    fn validate_rejects() {
        let d = load("eng", None).unwrap();
        for id in ["Bad_Id", "a/b", "", "1a", "é", "qa-2", "qa-02"] {
            let mut bad = d.clone();
            bad.roles[1].id = id.into();
            assert!(validate(&bad).is_err(), "{id}");
        }
        let mut bad = d.clone();
        bad.roles[1].id = "ba".into();
        assert!(validate(&bad).is_err());
        let mut bad = d.clone();
        bad.roles.remove(0);
        assert!(validate(&bad).is_err());
        let mut bad = d;
        bad.name = "status".into();
        assert!(validate(&bad).is_err());
    }

    #[test]
    fn zero_limits_rejected() {
        for name in ["instancesPerRole", "workersPerTicket", "ticketsPerBa"] {
            let json = format!(r#"{{"limits":{{"{name}":0}}}}"#);
            assert_eq!(
                load("eng", Some(&json)).unwrap_err(),
                format!("limits.{name} must be at least 1; set a positive number")
            );
        }
    }

    #[test]
    fn ask_permission_rejected() {
        let json = r#"{"roles":[{"id":"ba","permissions":[{"action":"read","resource":"x","effect":"ask"}]}]}"#;
        assert!(load("eng", Some(json)).unwrap_err().contains("ask"));
    }

    #[test]
    fn unknown_floor_remove_rejected() {
        assert_eq!(
            load("eng", Some(r#"{"floor":{"remove":["gh api"]}}"#)).unwrap_err(),
            "floor.remove: unknown floor rule 'gh api'"
        );
        assert!(load("eng", Some(r#"{"floor":{"remove":["gh api*","*.env"]}}"#)).is_ok());
    }

    #[test]
    fn ba_routes_implemented_work_to_reviewers() {
        assert!(
            load("eng", None).unwrap().roles[0]
                .boundary
                .contains("send implemented work to reviewer and qa afterward")
        );
    }
}
