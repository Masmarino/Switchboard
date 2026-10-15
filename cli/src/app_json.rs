use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use switchboard_core::control::RuntimeState;
use switchboard_core::process_manager::build_commands;
use switchboard_core::{AppConfig, AppDraft, AppKind};

use crate::args::AppFields;
use crate::error::CliError;

pub const FIELDS: [&str; 8] =
    ["name", "working_dir", "kind", "command", "url", "env_vars", "auto_restart", "start_order"];

/// Forme stable d'une app dans la sortie JSON du CLI.
pub fn app_json(app: &AppConfig, runtime: Option<&RuntimeState>) -> Value {
    json!({
        "id": app.id,
        "name": app.name,
        "kind": app.kind.ffi_str(),
        "working_dir": app.working_dir.display().to_string(),
        "command": app.command,
        "url": app.url,
        "env_vars": app.env_vars,
        "auto_restart": app.auto_restart,
        "start_order": app.start_order,
        "launch": launch_commands(app),
        "runtime": runtime_json(runtime),
    })
}

/// Sans superviseur, rien ne tourne : l'etat est "stopped".
pub fn runtime_json(runtime: Option<&RuntimeState>) -> Value {
    match runtime {
        Some(rt) => json!({
            "status": rt.status,
            "active": rt.active,
            "error": rt.error,
            "healthy": rt.healthy,
            "cpu_percent": rt.cpu_percent,
            "memory_mb": rt.memory_mb,
            "restarting": rt.restarting,
            "queued": rt.queued,
        }),
        None => json!({
            "status": "stopped",
            "active": false,
            "error": null,
            "healthy": null,
            "cpu_percent": 0.0,
            "memory_mb": 0.0,
            "restarting": false,
            "queued": false,
        }),
    }
}

/// Commandes reellement executees (dans un shell de connexion, depuis `working_dir`).
pub fn launch_commands(app: &AppConfig) -> Vec<String> {
    build_commands(app)
        .into_iter()
        .map(|spec| match app.kind {
            AppKind::Raw => spec.args.last().cloned().unwrap_or_default(),
            _ => std::iter::once(spec.program)
                .chain(spec.args.iter().map(|a| display_word(a)))
                .collect::<Vec<_>>()
                .join(" "),
        })
        .collect()
}

/// Remet des guillemets autour d'un argument qui en avait besoin, pour un affichage relisible.
fn display_word(word: &str) -> String {
    if !word.is_empty() && !word.chars().any(|c| c.is_whitespace() || "'\"\\$`".contains(c)) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

pub fn status_of(runtime: Option<&RuntimeState>) -> String {
    runtime.map(|rt| rt.status.clone()).unwrap_or_else(|| "stopped".to_string())
}

/// Modifications demandees par `--from-json` et/ou les options ; `None` = champ inchange.
#[derive(Debug, Default)]
pub struct FieldPatch {
    pub name: Option<String>,
    pub working_dir: Option<PathBuf>,
    pub kind: Option<AppKind>,
    pub command: Option<String>,
    pub url: Option<Option<String>>,
    pub env_vars: Option<Vec<(String, String)>>,
    pub env_set: Vec<(String, String)>,
    pub env_unset: Vec<String>,
    pub auto_restart: Option<bool>,
    pub start_order: Option<i32>,
}

impl FieldPatch {
    pub fn from_fields(fields: &AppFields, cwd: &Path) -> Result<Self, CliError> {
        let mut patch = match &fields.from_json {
            Some(source) => Self::from_json(&read_json_arg(source)?, cwd)?,
            None => Self::default(),
        };
        if let Some(name) = &fields.name {
            patch.name = Some(name.clone());
        }
        if let Some(dir) = &fields.dir {
            patch.working_dir = Some(resolve_dir(dir, cwd));
        }
        if let Some(kind) = &fields.kind {
            patch.kind = Some(parse_kind(kind)?);
        }
        if let Some(command) = &fields.command {
            patch.command = Some(command.clone());
        }
        if let Some(url) = &fields.url {
            patch.url = Some(Some(url.clone()));
        }
        for pair in &fields.env {
            patch.env_set.push(parse_env_pair(pair)?);
        }
        if let Some(auto_restart) = fields.auto_restart {
            patch.auto_restart = Some(auto_restart);
        }
        if let Some(order) = fields.start_order {
            patch.start_order = Some(order);
        }
        Ok(patch)
    }

    pub fn from_json(value: &Value, cwd: &Path) -> Result<Self, CliError> {
        let Value::Object(map) = value else {
            return Err(CliError::invalid_input("--from-json expects a JSON object"));
        };
        let mut patch = Self::default();
        for (key, value) in map {
            match key.as_str() {
                "name" => patch.name = Some(expect_string(key, value)?),
                "working_dir" => patch.working_dir = Some(resolve_dir(Path::new(&expect_string(key, value)?), cwd)),
                "kind" => patch.kind = Some(parse_kind(&expect_string(key, value)?)?),
                "command" => patch.command = Some(expect_string(key, value)?),
                "url" => patch.url = Some(if value.is_null() { None } else { Some(expect_string(key, value)?) }),
                "env_vars" => patch.env_vars = Some(parse_env_value(value)?),
                "auto_restart" => {
                    patch.auto_restart = Some(value.as_bool().ok_or_else(|| wrong_type(key, "a boolean"))?)
                }
                "start_order" => {
                    let order = value.as_i64().ok_or_else(|| wrong_type(key, "an integer"))?;
                    patch.start_order = Some(i32::try_from(order).map_err(|_| wrong_type(key, "a 32-bit integer"))?);
                }
                "id" => return Err(CliError::invalid_input("\"id\" cannot be set: ids are generated and never change")),
                other => {
                    return Err(CliError::invalid_input(format!("unknown field \"{other}\""))
                        .with_hint(format!("allowed fields: {}", FIELDS.join(", "))));
                }
            }
        }
        Ok(patch)
    }

    pub fn apply(&self, draft: &mut AppDraft) {
        if let Some(name) = &self.name {
            draft.name = name.clone();
        }
        if let Some(dir) = &self.working_dir {
            draft.working_dir = dir.clone();
        }
        if let Some(kind) = self.kind {
            draft.kind = kind;
        }
        if let Some(command) = &self.command {
            draft.command = command.clone();
        }
        if let Some(url) = &self.url {
            draft.url = url.clone();
        }
        if let Some(env_vars) = &self.env_vars {
            draft.env_vars = env_vars.clone();
        }
        draft.env_vars.retain(|(k, _)| !self.env_unset.iter().any(|u| u.trim() == k.trim()));
        for (key, value) in &self.env_set {
            match draft.env_vars.iter_mut().find(|(k, _)| k.trim() == key) {
                Some(existing) => existing.1 = value.clone(),
                None => draft.env_vars.push((key.clone(), value.clone())),
            }
        }
        if let Some(auto_restart) = self.auto_restart {
            draft.auto_restart = auto_restart;
        }
        if let Some(order) = self.start_order {
            draft.start_order = order;
        }
    }
}

/// Champs differents entre deux versions d'une app, dans l'ordre du schema.
pub fn changed_fields(before: &AppDraft, after: &AppDraft) -> Vec<&'static str> {
    let mut changed = Vec::new();
    let checks = [
        before.name != after.name,
        before.working_dir != after.working_dir,
        before.kind != after.kind,
        before.command != after.command,
        before.url != after.url,
        before.env_vars != after.env_vars,
        before.auto_restart != after.auto_restart,
        before.start_order != after.start_order,
    ];
    for (field, differs) in FIELDS.iter().zip(checks) {
        if differs {
            changed.push(*field);
        }
    }
    changed
}

pub fn parse_kind(value: &str) -> Result<AppKind, CliError> {
    AppKind::parse(value).ok_or_else(|| {
        let allowed: Vec<_> = AppKind::ALL.iter().map(|k| k.ffi_str()).collect();
        CliError::invalid_input(format!("unknown kind \"{value}\"")).with_hint(format!("allowed kinds: {}", allowed.join(", ")))
    })
}

pub fn parse_env_pair(pair: &str) -> Result<(String, String), CliError> {
    match pair.split_once('=') {
        Some((key, value)) if !key.trim().is_empty() => Ok((key.trim().to_string(), value.to_string())),
        _ => Err(CliError::invalid_input(format!("expected KEY=VALUE, got \"{pair}\""))),
    }
}

/// `~` developpe, chemin relatif resolu depuis le dossier courant du CLI.
pub fn resolve_dir(path: &Path, cwd: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let trimmed = text.trim();
    let expanded = match (trimmed.strip_prefix('~'), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            PathBuf::from(home).join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(trimmed),
    };
    if expanded.as_os_str().is_empty() || expanded.is_absolute() {
        expanded
    } else {
        normalize_lexically(&cwd.join(expanded))
    }
}

/// Retire `.` et resout `..` sans toucher au disque (le dossier peut ne pas exister encore).
fn normalize_lexically(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `-` = stdin, `@chemin` = fichier, sinon le JSON lui-meme.
pub fn read_json_arg(source: &str) -> Result<Value, CliError> {
    let text = if source == "-" {
        let mut buffer = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buffer).map_err(|e| CliError::io("stdin", e))?;
        buffer
    } else if let Some(path) = source.strip_prefix('@') {
        std::fs::read_to_string(path).map_err(|e| CliError::io(path, e))?
    } else {
        source.to_string()
    };
    serde_json::from_str(&text).map_err(|e| CliError::invalid_input(format!("invalid JSON: {e}")))
}

fn expect_string(key: &str, value: &Value) -> Result<String, CliError> {
    value.as_str().map(str::to_string).ok_or_else(|| wrong_type(key, "a string"))
}

fn wrong_type(key: &str, expected: &str) -> CliError {
    CliError::invalid_input(format!("field \"{key}\" must be {expected}"))
}

/// `[["KEY","value"], ...]` (format du fichier) ou `{"KEY": "value"}`.
fn parse_env_value(value: &Value) -> Result<Vec<(String, String)>, CliError> {
    let invalid = || wrong_type("env_vars", "an array of [key, value] pairs or an object of strings");
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| match item.as_array().map(Vec::as_slice) {
                Some([Value::String(k), Value::String(v)]) => Ok((k.clone(), v.clone())),
                _ => Err(invalid()),
            })
            .collect(),
        Value::Object(map) => map_to_pairs(map).ok_or_else(invalid),
        Value::Null => Ok(Vec::new()),
        _ => Err(invalid()),
    }
}

fn map_to_pairs(map: &Map<String, Value>) -> Option<Vec<(String, String)>> {
    map.iter().map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cwd() -> PathBuf {
        PathBuf::from("/work/project")
    }

    #[test]
    fn from_json_reads_every_field() {
        let value = json!({
            "name": "Api", "working_dir": "api", "kind": "Npm", "command": "run dev",
            "url": "http://localhost:3000", "env_vars": {"PORT": "3000"}, "auto_restart": true, "start_order": 2
        });
        let patch = FieldPatch::from_json(&value, &cwd()).unwrap();
        let mut draft = AppDraft::default();
        patch.apply(&mut draft);
        assert_eq!(draft.name, "Api");
        assert_eq!(draft.working_dir, PathBuf::from("/work/project/api"));
        assert_eq!(draft.kind, AppKind::Npm);
        assert_eq!(draft.command, "run dev");
        assert_eq!(draft.url.as_deref(), Some("http://localhost:3000"));
        assert_eq!(draft.env_vars, vec![("PORT".to_string(), "3000".to_string())]);
        assert!(draft.auto_restart);
        assert_eq!(draft.start_order, 2);
    }

    #[test]
    fn from_json_rejects_unknown_fields_ids_and_wrong_types() {
        assert!(FieldPatch::from_json(&json!({"nmae": "x"}), &cwd()).unwrap_err().hint.unwrap().contains("working_dir"));
        assert!(FieldPatch::from_json(&json!({"id": "x"}), &cwd()).is_err());
        assert!(FieldPatch::from_json(&json!({"start_order": "1"}), &cwd()).is_err());
        assert!(FieldPatch::from_json(&json!({"env_vars": [["A"]]}), &cwd()).is_err());
        assert!(FieldPatch::from_json(&json!(["not", "an", "object"]), &cwd()).is_err());
    }

    #[test]
    fn null_url_clears_it() {
        let patch = FieldPatch::from_json(&json!({"url": null}), &cwd()).unwrap();
        let mut draft = AppDraft { url: Some("http://x".to_string()), ..Default::default() };
        patch.apply(&mut draft);
        assert_eq!(draft.url, None);
    }

    #[test]
    fn env_flags_replace_keys_and_unset_removes_them() {
        let mut draft = AppDraft {
            env_vars: vec![("A".to_string(), "1".to_string()), ("B".to_string(), "2".to_string())],
            ..Default::default()
        };
        let patch = FieldPatch {
            env_set: vec![("A".to_string(), "10".to_string()), ("C".to_string(), "3".to_string())],
            env_unset: vec!["B".to_string()],
            ..Default::default()
        };
        patch.apply(&mut draft);
        assert_eq!(
            draft.env_vars,
            vec![("A".to_string(), "10".to_string()), ("C".to_string(), "3".to_string())]
        );
    }

    #[test]
    fn env_pairs_keep_equal_signs_in_values() {
        assert_eq!(parse_env_pair("URL=a=b").unwrap(), ("URL".to_string(), "a=b".to_string()));
        assert!(parse_env_pair("=x").is_err());
        assert!(parse_env_pair("NOVALUE").is_err());
    }

    #[test]
    fn dirs_are_made_absolute() {
        assert_eq!(resolve_dir(Path::new("./api/../web"), &cwd()), PathBuf::from("/work/project/web"));
        assert_eq!(resolve_dir(Path::new("/abs"), &cwd()), PathBuf::from("/abs"));
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(resolve_dir(Path::new("~/code"), &cwd()), PathBuf::from(home).join("code"));
        }
    }

    #[test]
    fn launch_commands_show_what_runs() {
        let cargo = AppConfig { kind: AppKind::Cargo, ..Default::default() };
        assert_eq!(launch_commands(&cargo), vec!["cargo build", "cargo run"]);
        let raw = AppConfig { kind: AppKind::Raw, command: "./run.sh --x".to_string(), ..Default::default() };
        assert_eq!(launch_commands(&raw), vec!["./run.sh --x"]);
        let python = AppConfig { kind: AppKind::Python, command: r#"-c "print('hi')""#.to_string(), ..Default::default() };
        assert_eq!(launch_commands(&python), vec![r#"python3 -c 'print('\''hi'\'')'"#]);
    }

    #[test]
    fn changed_fields_lists_differences_in_schema_order() {
        let before = AppDraft::default();
        let after = AppDraft { start_order: 3, name: "x".to_string(), ..Default::default() };
        assert_eq!(changed_fields(&before, &after), vec!["name", "start_order"]);
    }
}
