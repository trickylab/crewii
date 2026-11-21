use crate::{Result, opencode::Api, paths};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Models {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<String>,
}

pub struct Entry {
    pub id: String,
    pub name: String,
    pub variants: Vec<String>,
}

pub fn local_path(root: &Path) -> PathBuf {
    root.join(".opencode/crewii/state/models.json")
}

pub fn global_path(var: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf> {
    paths::config_dir(var)
        .map(|p| p.join("config.json"))
        .map_err(|e| format!("Cannot locate global config; {e}"))
}

fn read_json(path: &Path) -> Result<Option<Value>> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "Cannot read {}: {e}; check file permissions",
                path.display()
            ));
        }
    };
    serde_json::from_slice(&data)
        .map(Some)
        .map_err(|e| format!("Invalid JSON in {}: {e}; fix the file", path.display()))
}

fn parse_models(value: Value, path: &Path) -> Result<Models> {
    serde_json::from_value(value).map_err(|e| {
        format!(
            "Invalid models in {}: {e}; use primary and subagent fields",
            path.display()
        )
    })
}

pub fn read_local(path: &Path) -> Result<Option<Models>> {
    read_json(path)?.map(|v| parse_models(v, path)).transpose()
}

fn global_object(path: &Path) -> Result<Option<Map<String, Value>>> {
    read_json(path)?
        .map(|v| {
            v.as_object().cloned().ok_or_else(|| {
                format!(
                    "Invalid global config in {}; use a JSON object",
                    path.display()
                )
            })
        })
        .transpose()
}

pub fn read_global(path: &Path) -> Result<Option<Models>> {
    global_object(path)?
        .and_then(|mut v| v.remove("models"))
        .map(|v| parse_models(v, path))
        .transpose()
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| format!("Cannot encode models: {e}; check the configuration"))?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Cannot create {}: {e}; check directory permissions",
                parent.display()
            )
        })?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| {
        format!(
            "Cannot open {}: {e}; check file permissions",
            path.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| {
                format!(
                    "Cannot secure {}: {e}; check file permissions",
                    path.display()
                )
            })?;
    }
    file.set_len(0)
        .and_then(|_| file.write_all(&bytes))
        .map_err(|e| {
            format!(
                "Cannot write {}: {e}; check disk space and permissions",
                path.display()
            )
        })
}

pub fn write_local(path: &Path, m: &Models) -> Result<()> {
    write_json(path, m)
}

pub fn write_global(path: &Path, m: &Models) -> Result<()> {
    let mut object = global_object(path)?.unwrap_or_default();
    object.insert(
        "models".into(),
        serde_json::to_value(m)
            .map_err(|e| format!("Cannot encode models: {e}; check the configuration"))?,
    );
    write_json(path, &object)
}

pub fn effective(local: Option<&Models>, global: Option<&Models>) -> Models {
    fn field(local: Option<&String>, global: Option<&String>) -> Option<String> {
        local
            .filter(|v| !v.trim().is_empty())
            .or_else(|| global.filter(|v| !v.trim().is_empty()))
            .cloned()
    }
    Models {
        primary: field(
            local.and_then(|m| m.primary.as_ref()),
            global.and_then(|m| m.primary.as_ref()),
        ),
        subagent: field(
            local.and_then(|m| m.subagent.as_ref()),
            global.and_then(|m| m.subagent.as_ref()),
        ),
    }
}

pub fn catalog(api: &dyn Api, dir: &Path) -> Result<Vec<Entry>> {
    let data = api.get(dir, "/api/model")?;
    let models = data
        .as_array()
        .ok_or_else(|| "Invalid model catalog; check the OpenCode version".to_owned())?;
    let mut entries = Vec::new();
    for model in models {
        if model.get("enabled").and_then(Value::as_bool) == Some(false)
            || model
                .pointer("/capabilities/tools")
                .and_then(Value::as_bool)
                != Some(true)
        {
            continue;
        }
        let string = |key| {
            model.get(key).and_then(Value::as_str).ok_or_else(|| {
                format!("Invalid model catalog: missing {key}; check the OpenCode version")
            })
        };
        entries.push(Entry {
            id: format!("{}/{}", string("providerID")?, string("modelID")?),
            name: string("name")?.to_owned(),
            variants: model
                .get("variants")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| v.get("id").and_then(Value::as_str).map(str::to_owned))
                .collect(),
        });
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(entries)
}

pub fn validate(catalog: &[Entry], id: &str) -> Result<()> {
    let (base, variant) = id
        .split_once('#')
        .map_or((id, None), |(base, variant)| (base, Some(variant)));
    if catalog
        .iter()
        .any(|e| e.id == base && variant.is_none_or(|v| e.variants.iter().any(|x| x == v)))
    {
        Ok(())
    } else {
        Err(format!(
            "Unknown model '{id}'; run crewii models to pick one"
        ))
    }
}

pub fn pick(
    catalog: &[Entry],
    label: &str,
    current: Option<&str>,
    ask: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let mut no_match = false;
    loop {
        let prefix = if no_match {
            "No matches; try again.\n"
        } else {
            ""
        };
        let Some(input) = ask(&format!(
            "{prefix}{label} model [Enter = {}]: ",
            current.unwrap_or("OpenCode default")
        )) else {
            return current.map(str::to_owned);
        };
        if input.is_empty() {
            return current.map(str::to_owned);
        }
        if validate(catalog, &input).is_ok() {
            return Some(input);
        }
        let (filter, wanted) = input.split_once('#').unwrap_or((&input, ""));
        let filter = filter.to_lowercase();
        let matches: Vec<_> = catalog
            .iter()
            .filter(|e| {
                e.id.to_lowercase().contains(&filter) || e.name.to_lowercase().contains(&filter)
            })
            .take(20)
            .collect();
        if matches.is_empty() {
            no_match = true;
            continue;
        }
        let list = matches
            .iter()
            .enumerate()
            .map(|(i, e)| format!("  {}) {}\n", i + 1, e.id))
            .collect::<String>();
        let entry = loop {
            let Some(input) = ask(&format!("{list}Pick [1-{}]: ", matches.len())) else {
                return current.map(str::to_owned);
            };
            if let Some(entry) = input
                .parse::<usize>()
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| matches.get(i))
            {
                break *entry;
            }
        };
        if entry.variants.is_empty() {
            return Some(entry.id.clone());
        }
        if entry.variants.iter().any(|v| v == wanted) {
            return Some(format!("{}#{wanted}", entry.id));
        }
        loop {
            let Some(variant) = ask(&format!(
                "Variant ({}) [Enter = none]: ",
                entry.variants.join("/")
            )) else {
                return current.map(str::to_owned);
            };
            if variant.is_empty() {
                return Some(entry.id.clone());
            }
            if entry.variants.contains(&variant) {
                return Some(format!("{}#{variant}", entry.id));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opencode::Fake;
    use serde_json::json;
    use std::{cell::RefCell, collections::VecDeque, fs, path::Path};

    fn choice(
        inputs: &[Option<&str>],
        entries: &[Entry],
        current: Option<&str>,
    ) -> (Option<String>, Vec<String>) {
        let inputs = RefCell::new(
            inputs
                .iter()
                .map(|s| s.map(str::to_owned))
                .collect::<VecDeque<_>>(),
        );
        let prompts = RefCell::new(Vec::new());
        let result = pick(entries, "Primary", current, &|prompt| {
            prompts.borrow_mut().push(prompt.to_owned());
            inputs.borrow_mut().pop_front().expect("unexpected prompt")
        });
        (result, prompts.into_inner())
    }

    fn entries() -> Vec<Entry> {
        vec![Entry {
            id: "p/m".into(),
            name: "Special Model".into(),
            variants: vec!["high".into(), "low".into()],
        }]
    }

    #[test]
    fn catalog_filters_sorts_and_uses_data_array() {
        let api = Fake::new();
        api.on("get", "/api/model", json!([
            {"providerID":"z", "modelID":"b", "name":"Bee", "enabled":true, "type":null, "capabilities":{"tools":true}, "variants":[{"id":"high"}]},
            {"providerID":"a", "modelID":"a", "name":"Aye", "capabilities":{"tools":true}, "variants":[]},
            {"providerID":"x", "modelID":"disabled", "name":"X", "enabled":false, "capabilities":{"tools":true}},
            {"providerID":"x", "modelID":"no-tools", "name":"X", "enabled":true, "capabilities":{"tools":false}}
        ]));
        let found = catalog(&api, Path::new("/project")).unwrap();
        assert_eq!(
            found.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["a/a", "z/b"]
        );
        assert_eq!(found[1].variants, ["high"]);
        assert_eq!(found[1].name, "Bee");
        assert_eq!(api.calls.borrow()[0].2, "/api/model");
        assert!(catalog(&Fake::new(), Path::new("/")).is_err());
        api.on("get", "/api/model", json!({}));
        assert!(catalog(&api, Path::new("/")).is_err());
    }

    #[test]
    fn validate_models_and_variants() {
        for id in ["p/m", "p/m#high"] {
            assert!(validate(&entries(), id).is_ok());
        }
        for id in ["bad", "p/m#bad", "p/m#", "p/m#high#x"] {
            let error = validate(&entries(), id).unwrap_err();
            assert!(error.contains("crewii models"), "{error}");
        }
    }

    #[test]
    fn picker_defaults_and_eof_at_every_stage() {
        for input in [
            vec![Some("")],
            vec![None],
            vec![Some("special"), None],
            vec![Some("p/"), Some("1"), None],
        ] {
            assert_eq!(
                choice(&input, &entries(), Some("old")).0.as_deref(),
                Some("old")
            );
        }
        assert_eq!(choice(&[None], &entries(), None).0, None);
        assert!(choice(&[None], &entries(), None).1[0].contains("OpenCode default"));
    }

    #[test]
    fn picker_retries_filter_number_variant_and_limits_list() {
        let (result, prompts) = choice(
            &[
                Some("missing"),
                Some("sPeCiAl"),
                Some("x"),
                Some("0"),
                Some("2"),
                Some("1"),
                Some("bad"),
                Some("high"),
            ],
            &entries(),
            None,
        );
        assert_eq!(result.as_deref(), Some("p/m#high"));
        assert!(
            prompts
                .iter()
                .any(|p| p.to_lowercase().contains("no match"))
        );
        assert!(prompts.iter().any(|p| p.contains("  1) p/m")));
        assert_eq!(
            choice(&[Some("p"), Some("1"), Some("")], &entries(), None)
                .0
                .as_deref(),
            Some("p/m")
        );
        let many: Vec<_> = (0..25)
            .map(|i| Entry {
                id: format!("p/{i:02}"),
                name: String::new(),
                variants: vec![],
            })
            .collect();
        let (result, prompts) = choice(&[Some("p/"), Some("21"), Some("20")], &many, None);
        assert_eq!(result.as_deref(), Some("p/19"));
        let all = prompts.join("");
        assert!(all.contains("20) p/19"));
        assert!(!all.contains("21) p/20"));
    }

    #[test]
    fn picker_accepts_exact_ids_and_search_variants() {
        for id in ["p/m#high", "p/m"] {
            let (result, prompts) = choice(&[Some(id)], &entries(), Some("p/m#high"));
            assert_eq!(result.as_deref(), Some(id));
            assert_eq!(prompts.len(), 1);
        }
        let (result, prompts) = choice(&[Some("special#low"), Some("1")], &entries(), None);
        assert_eq!(result.as_deref(), Some("p/m#low"));
        assert!(!prompts.iter().any(|p| p.contains("Variant")));
        let (result, prompts) = choice(
            &[Some("special#bad"), Some("1"), Some("high")],
            &entries(),
            None,
        );
        assert_eq!(result.as_deref(), Some("p/m#high"));
        assert!(prompts.iter().any(|p| p.contains("Variant")));
    }

    #[test]
    fn effective_uses_nonblank_per_field() {
        let local = Models {
            primary: Some("  ".into()),
            subagent: Some("local".into()),
        };
        let global = Models {
            primary: Some("global".into()),
            subagent: Some("other".into()),
        };
        assert_eq!(
            effective(Some(&local), Some(&global)),
            Models {
                primary: Some("global".into()),
                subagent: Some("local".into())
            }
        );
        assert_eq!(
            effective(
                None,
                Some(&Models {
                    primary: Some(" \t".into()),
                    subagent: None
                })
            ),
            Models::default()
        );
        assert_eq!(effective(None, None), Models::default());
    }

    #[test]
    fn paths_storage_round_trip_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let local = local_path(dir.path());
        assert!(local.ends_with(".opencode/crewii/state/models.json"));
        assert!(global_path(&|_| None).is_err());
        assert_eq!(
            global_path(&|k| (k == "XDG_CONFIG_HOME").then(|| "/config".into())).unwrap(),
            Path::new("/config/crewii/config.json")
        );
        assert_eq!(read_local(&local).unwrap(), None);
        assert_eq!(read_global(&local).unwrap(), None);
        let m = Models {
            primary: Some("p/m".into()),
            subagent: None,
        };
        write_local(&local, &m).unwrap();
        assert_eq!(read_local(&local).unwrap(), Some(m.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&local).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let global = dir.path().join("global/config.json");
        write_global(&global, &m).unwrap();
        assert_eq!(read_global(&global).unwrap(), Some(m.clone()));
        fs::write(
            &global,
            r#"{"other":{"keep":true},"models":{"primary":"old"}}"#,
        )
        .unwrap();
        write_global(&global, &m).unwrap();
        let data: serde_json::Value = serde_json::from_slice(&fs::read(&global).unwrap()).unwrap();
        assert_eq!(data["other"]["keep"], true);
        assert_eq!(read_global(&global).unwrap(), Some(m));
        fs::write(&global, r#"{"models":{"strong":"x"}}"#).unwrap();
        let error = read_global(&global).unwrap_err();
        assert!(error.contains("primary") && error.contains("subagent"));
        fs::write(&global, "{}").unwrap();
        assert_eq!(read_global(&global).unwrap(), None);
        fs::write(&global, "[").unwrap();
        assert!(read_global(&global).is_err());
        assert!(write_global(&global, &Models::default()).is_err());
        fs::write(&global, "[]").unwrap();
        assert!(read_global(&global).is_err());
        assert!(write_global(&global, &Models::default()).is_err());
        fs::write(&local, "bad").unwrap();
        assert!(read_local(&local).is_err());
        assert!(read_local(dir.path()).is_err());
        assert!(write_local(dir.path(), &Models::default()).is_err());
    }
}
