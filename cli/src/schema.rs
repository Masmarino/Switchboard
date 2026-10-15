use serde_json::{json, Value};
use switchboard_core::process_manager::build_commands;
use switchboard_core::validation::START_ORDER_MAX;
use switchboard_core::{AppConfig, AppKind, CommandRule};

use crate::error::ErrorCode;

/// Incremente si un champ du schema ou de l'enveloppe JSON change de sens.
pub const SCHEMA_VERSION: u32 = 1;

pub fn schema_json(config_path: &str) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "cli_version": env!("CARGO_PKG_VERSION"),
        "config_file": {
            "path": config_path,
            "format": "{\"apps\": [ {id, name, working_dir, command, kind, url, env_vars, auto_restart, start_order} ]}",
            "notes": "Shared with the native apps, which reload it within a second. The file spells kinds in PascalCase (\"Npm\"); the CLI prints them lowercase and accepts both.",
        },
        "app_reference": "An app id, a unique id prefix of at least 4 characters, or the app name (case-insensitive).",
        "fields": fields(),
        "kinds": AppKind::ALL.iter().map(|k| kind_json(*k)).collect::<Vec<_>>(),
        "statuses": {
            "stopped": "not running (never started, exited cleanly, or stopped)",
            "building": "cargo only: `cargo build` is running",
            "running": "the process is up",
            "failed": "exited with an error or could not start; see runtime.error and logs",
        },
        "health": "runtime.healthy is null without a url or while stopped; otherwise true when the url answers with a status below 500 (checked every 5 s).",
        "exit_codes": exit_codes(),
        "error_codes": ErrorCode::ALL.iter().map(|c| json!({
            "code": c.as_str(),
            "exit_code": c.exit_code(),
            "meaning": c.description(),
        })).collect::<Vec<_>>(),
    })
}

fn fields() -> Value {
    json!([
        {
            "name": "id", "type": "uuid", "required": false, "read_only": true,
            "description": "Generated on creation, never changes.",
        },
        {
            "name": "name", "type": "string", "required": true, "flag": "--name",
            "rules": ["not empty after trimming", "unique among apps (case-insensitive)"],
            "description": "Display name, also usable as an app reference.",
        },
        {
            "name": "working_dir", "type": "path", "required": true, "flag": "--dir",
            "rules": ["absolute (the CLI resolves relative paths and ~ against its current directory)", "warning if it does not exist"],
            "description": "Directory the command runs in.",
        },
        {
            "name": "kind", "type": "enum", "required": true, "flag": "--kind",
            "values": AppKind::ALL.iter().map(|k| k.ffi_str()).collect::<Vec<_>>(),
            "description": "Toolchain; decides the program launched and what `command` means (see kinds).",
        },
        {
            "name": "command", "type": "string", "required": "depends on kind", "flag": "--command", "default": "",
            "rules": [
                "required for npm and raw",
                "ignored for cargo",
                "split like a shell: '...' and \"...\" keep spaces; an unclosed quote is rejected (raw commands go to sh -c as is)",
            ],
            "description": "Arguments appended to the kind's program; for raw, a full shell command.",
        },
        {
            "name": "url", "type": "string|null", "required": false, "flag": "--url (edit: --no-url to remove)", "default": null,
            "rules": ["http:// or https://, no spaces"],
            "description": "Local URL used for health checks and 'open in browser'.",
        },
        {
            "name": "env_vars", "type": "array of [key, value]", "required": false,
            "flag": "--env KEY=VALUE (repeatable), edit: --unset-env KEY, --clear-env", "default": [],
            "rules": ["keys: no spaces, '=' or NUL, no duplicates", "keys and values are trimmed like in the native form"],
            "json_input": "[[\"KEY\", \"value\"]] or {\"KEY\": \"value\"}",
            "description": "Environment variables added to the process environment.",
        },
        {
            "name": "auto_restart", "type": "boolean", "required": false, "flag": "--auto-restart [true|false]", "default": false,
            "description": "Relaunch after a crash (non-zero exit not caused by a stop).",
        },
        {
            "name": "start_order", "type": "integer", "required": false, "flag": "--start-order N", "default": 0,
            "rules": [format!("between 0 and {START_ORDER_MAX}")],
            "description": "Tier for `start --all`: lower tiers start first, equal values start together.",
        },
    ])
}

fn kind_json(kind: AppKind) -> Value {
    let rule = kind.command_rule();
    let sample = AppConfig { kind, command: "<command>".to_string(), ..Default::default() };
    let launches: Vec<String> = build_commands(&sample)
        .into_iter()
        .map(|spec| std::iter::once(spec.program).chain(spec.args).collect::<Vec<_>>().join(" "))
        .collect();
    json!({
        "value": kind.ffi_str(),
        "config_value": kind.display_name(),
        "command": rule.as_str(),
        "default_command": match rule {
            CommandRule::Optional { default } => default,
            _ => None,
        },
        "launches": launches,
    })
}

fn exit_codes() -> Value {
    json!([
        { "exit_code": 0, "meaning": "success" },
        { "exit_code": 1, "meaning": "io_error, internal" },
        { "exit_code": 2, "meaning": "usage, invalid_input" },
        { "exit_code": 3, "meaning": "app_not_found, ambiguous_app" },
        { "exit_code": 4, "meaning": "validation_failed" },
        { "exit_code": 5, "meaning": "config_invalid" },
        { "exit_code": 6, "meaning": "no_supervisor, already_supervised, supervisor_error, daemon_start_failed" },
        { "exit_code": 7, "meaning": "timeout" },
        { "exit_code": 8, "meaning": "app_failed" },
    ])
}

pub fn schema_human(config_path: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("Config file: {config_path}\n\nFields:\n"));
    if let Some(fields) = fields().as_array() {
        for field in fields {
            out.push_str(&format!(
                "  {:<13} {:<22} {}\n",
                field["name"].as_str().unwrap_or_default(),
                field["type"].as_str().unwrap_or_default(),
                field["description"].as_str().unwrap_or_default()
            ));
        }
    }
    out.push_str("\nKinds:\n");
    for kind in AppKind::ALL {
        let value = kind_json(kind);
        out.push_str(&format!(
            "  {:<8} command {:<9} runs: {}\n",
            kind.ffi_str(),
            value["command"].as_str().unwrap_or_default(),
            value["launches"]
                .as_array()
                .map(|l| l.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" then "))
                .unwrap_or_default()
        ));
    }
    out.push_str("\nUse --json for the complete schema (rules, defaults, exit and error codes).");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_lists_every_kind_and_error_code() {
        let schema = schema_json("/tmp/apps.json");
        assert_eq!(schema["kinds"].as_array().unwrap().len(), AppKind::ALL.len());
        assert_eq!(schema["error_codes"].as_array().unwrap().len(), ErrorCode::ALL.len());
        let npm = &schema["kinds"][1];
        assert_eq!(npm["value"], "npm");
        assert_eq!(npm["command"], "required");
        assert_eq!(npm["launches"][0], "npm <command>");
    }

    #[test]
    fn schema_fields_match_the_patch_fields() {
        let schema = schema_json("/tmp/apps.json");
        let names: Vec<&str> =
            schema["fields"].as_array().unwrap().iter().filter_map(|f| f["name"].as_str()).filter(|n| *n != "id").collect();
        assert_eq!(names, crate::app_json::FIELDS.to_vec());
    }
}
