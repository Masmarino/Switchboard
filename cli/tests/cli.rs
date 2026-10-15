//! Tests de bout en bout du binaire, chacun dans son propre dossier de config temporaire.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

struct Sandbox {
    dir: tempfile::TempDir,
    project: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        // Le dossier courant vu par le CLI est canonique (/private/var/... sous macOS).
        let project = project.canonicalize().unwrap();
        Self { dir, project }
    }

    fn config_dir(&self) -> PathBuf {
        self.dir.path().join("config")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_switchboard-cli"));
        command
            .arg("--config-dir")
            .arg(self.config_dir())
            .args(args)
            .current_dir(&self.project)
            // Pas de .zshrc de l'utilisateur dans les tests : plus rapide et reproductible.
            .env("SHELL", "/bin/sh")
            .env_remove("SWITCHBOARD_JSON")
            .env_remove("SWITCHBOARD_CONFIG_DIR");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run switchboard-cli")
    }

    /// Lance avec `--json` et retourne (code de sortie, enveloppe).
    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let output = self.run(&all);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
            panic!("invalid JSON from {args:?}: {e}\nstdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&output.stderr))
        });
        (output.status.code().unwrap_or(-1), value)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, value) = self.json(args);
        assert_eq!(code, 0, "{args:?} failed: {value:#}");
        assert_eq!(value["ok"], true);
        value["data"].clone()
    }

    fn add_raw(&self, name: &str, command: &str) -> String {
        let data = self.ok(&["add", "--name", name, "--dir", ".", "--kind", "raw", "--command", command]);
        data["app"]["id"].as_str().unwrap().to_string()
    }

    fn config_file(&self) -> PathBuf {
        self.config_dir().join("apps.json")
    }
}

/// Arrete le daemon eventuel meme si le test echoue, pour ne pas laisser de process.
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.run(&["daemon", "stop"]);
    }
}

fn error_code(value: &Value) -> &str {
    value["error"]["code"].as_str().unwrap_or_default()
}

#[test]
fn help_and_version_print_without_error() {
    let sandbox = Sandbox::new();
    let help = sandbox.run(&["--help"]);
    assert!(help.status.success());
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("EXIT CODES"));
    assert!(text.contains("schema"));
    let version = sandbox.run(&["--version"]);
    assert!(String::from_utf8_lossy(&version.stdout).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn usage_errors_are_json_with_exit_code_2() {
    let sandbox = Sandbox::new();
    let (code, value) = sandbox.json(&["add", "--bogus"]);
    assert_eq!(code, 2);
    assert_eq!(value["ok"], false);
    assert_eq!(error_code(&value), "usage");
}

#[test]
fn add_list_show_edit_remove_round_trip() {
    let sandbox = Sandbox::new();
    assert_eq!(sandbox.ok(&["list"])["apps"].as_array().unwrap().len(), 0);

    let added = sandbox.ok(&[
        "add", "--name", "Api", "--dir", ".", "--kind", "Npm", "--command", "run dev",
        "--url", "http://localhost:3000", "--env", "PORT=3000", "--auto-restart", "--start-order", "2",
    ]);
    let app = &added["app"];
    let id = app["id"].as_str().unwrap();
    assert_eq!(app["kind"], "npm");
    assert_eq!(app["working_dir"], sandbox.project.display().to_string());
    assert_eq!(app["env_vars"][0][0], "PORT");
    assert_eq!(app["auto_restart"], true);
    assert_eq!(app["launch"][0], "npm run dev");
    assert_eq!(app["runtime"]["status"], "stopped");

    // The file keeps the native apps' format.
    let file: Value = serde_json::from_str(&std::fs::read_to_string(sandbox.config_file()).unwrap()).unwrap();
    assert_eq!(file["apps"][0]["kind"], "Npm");

    let shown = sandbox.ok(&["show", "api"]);
    assert_eq!(shown["app"]["id"], id);
    assert_eq!(sandbox.ok(&["show", &id[..8]])["app"]["name"], "Api");

    let edited = sandbox.ok(&["edit", "Api", "--no-url", "--env", "PORT=4000", "--env", "DEBUG=1", "--auto-restart", "false"]);
    assert_eq!(edited["changed"], serde_json::json!(["url", "env_vars", "auto_restart"]));
    assert_eq!(edited["restart_required"], false);
    assert_eq!(edited["app"]["url"], Value::Null);
    assert_eq!(edited["app"]["env_vars"], serde_json::json!([["PORT", "4000"], ["DEBUG", "1"]]));

    let unchanged = sandbox.ok(&["edit", id, "--name", "Api"]);
    assert_eq!(unchanged["changed"], serde_json::json!([]));

    let removed = sandbox.ok(&["remove", "api"]);
    assert_eq!(removed["removed"][0]["id"], id);
    assert_eq!(sandbox.ok(&["list"])["apps"].as_array().unwrap().len(), 0);
}

#[test]
fn add_from_json_with_flag_override() {
    let sandbox = Sandbox::new();
    let json = r#"{"name":"Worker","working_dir":"sub","kind":"python","command":"worker.py","env_vars":{"A":"1"},"start_order":5}"#;
    let app = &sandbox.ok(&["add", "--from-json", json, "--start-order", "7"])["app"];
    assert_eq!(app["working_dir"], sandbox.project.join("sub").display().to_string());
    assert_eq!(app["start_order"], 7);
    assert_eq!(app["env_vars"], serde_json::json!([["A", "1"]]));

    let (code, value) = sandbox.json(&["add", "--from-json", r#"{"name":"X","colour":"red"}"#]);
    assert_eq!(code, 2);
    assert_eq!(error_code(&value), "invalid_input");
}

#[test]
fn validation_errors_list_every_problem_and_write_nothing() {
    let sandbox = Sandbox::new();
    sandbox.add_raw("Api", "true");
    let (code, value) = sandbox.json(&["add", "--name", "api", "--dir", ".", "--url", "localhost:3000", "--start-order", "120"]);
    assert_eq!(code, 4);
    assert_eq!(error_code(&value), "validation_failed");
    let fields: Vec<(&str, &str)> = value["error"]["details"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["field"].as_str().unwrap(), e["code"].as_str().unwrap()))
        .collect();
    assert_eq!(
        fields,
        vec![("kind", "required"), ("name", "duplicate"), ("url", "invalid"), ("start_order", "out_of_range")]
    );
    assert_eq!(sandbox.ok(&["list"])["apps"].as_array().unwrap().len(), 1);
}

#[test]
fn missing_directory_is_a_warning() {
    let sandbox = Sandbox::new();
    let (code, value) = sandbox.json(&["add", "--name", "Later", "--dir", "not-yet", "--kind", "go"]);
    assert_eq!(code, 0);
    assert_eq!(value["warnings"][0]["code"], "missing");
}

#[test]
fn unknown_and_ambiguous_references_exit_3() {
    let sandbox = Sandbox::new();
    sandbox.add_raw("Api", "true");
    let (code, value) = sandbox.json(&["show", "nope"]);
    assert_eq!(code, 3);
    assert_eq!(error_code(&value), "app_not_found");
    assert_eq!(value["error"]["details"]["known_apps"][0], "Api");
}

#[test]
fn an_invalid_config_file_is_reported_and_never_overwritten() {
    let sandbox = Sandbox::new();
    std::fs::create_dir_all(sandbox.config_dir()).unwrap();
    std::fs::write(sandbox.config_file(), "{ broken").unwrap();
    let (code, value) = sandbox.json(&["add", "--name", "X", "--dir", ".", "--kind", "raw", "--command", "true"]);
    assert_eq!(code, 5);
    assert_eq!(error_code(&value), "config_invalid");
    assert_eq!(std::fs::read_to_string(sandbox.config_file()).unwrap(), "{ broken");
}

#[test]
fn export_and_import_use_the_native_format() {
    let sandbox = Sandbox::new();
    sandbox.ok(&["add", "--name", "Api", "--dir", ".", "--kind", "cargo", "--env", "SECRET=1"]);
    let exported = sandbox.dir.path().join("export.json");
    sandbox.ok(&["export", "--output", exported.to_str().unwrap()]);
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&exported).unwrap()).unwrap();
    assert_eq!(file["apps"][0]["kind"], "Cargo");
    assert_eq!(file["apps"][0]["env_vars"], serde_json::json!([]), "env vars are excluded unless --include-env");

    let other = Sandbox::new();
    let preview = other.ok(&["import", exported.to_str().unwrap(), "--dry-run"]);
    assert_eq!(preview["added"], serde_json::json!(["Api"]));
    assert_eq!(other.ok(&["list"])["apps"].as_array().unwrap().len(), 0);
    other.ok(&["import", exported.to_str().unwrap()]);
    let replaced = other.ok(&["import", exported.to_str().unwrap()]);
    assert_eq!(replaced["replaced"], serde_json::json!(["Api"]));
    assert_eq!(other.ok(&["list"])["apps"].as_array().unwrap().len(), 1);
}

#[test]
fn import_rejects_invalid_apps_unless_told_otherwise() {
    let sandbox = Sandbox::new();
    let file = sandbox.dir.path().join("bad.json");
    std::fs::write(&file, r#"{"apps":[{"id":"550e8400-e29b-41d4-a716-446655440000","name":"Bad","working_dir":"/tmp","command":"","kind":"Npm"}]}"#).unwrap();
    let (code, value) = sandbox.json(&["import", file.to_str().unwrap()]);
    assert_eq!(code, 4);
    assert_eq!(value["error"]["details"]["errors"][0]["code"], "required");
    let forced = sandbox.ok(&["import", file.to_str().unwrap(), "--skip-validation"]);
    assert_eq!(forced["added"], serde_json::json!(["Bad"]));
}

#[test]
fn schema_describes_fields_kinds_and_codes() {
    let sandbox = Sandbox::new();
    let schema = sandbox.ok(&["schema"]);
    let fields: Vec<&str> = schema["fields"].as_array().unwrap().iter().filter_map(|f| f["name"].as_str()).collect();
    assert!(fields.contains(&"working_dir"));
    assert_eq!(schema["kinds"].as_array().unwrap().len(), 7);
    assert!(schema["error_codes"].as_array().unwrap().iter().any(|c| c["code"] == "timeout" && c["exit_code"] == 7));
}

#[test]
fn process_commands_without_a_supervisor() {
    let sandbox = Sandbox::new();
    sandbox.add_raw("Api", "true");
    let stopped = sandbox.ok(&["stop", "api"]);
    assert_eq!(stopped["results"][0]["outcome"], "not_running");
    assert_eq!(sandbox.ok(&["status"])["supervisor"], Value::Null);
    let (code, value) = sandbox.json(&["logs", "api"]);
    assert_eq!(code, 6);
    assert_eq!(error_code(&value), "no_supervisor");
    let (code, value) = sandbox.json(&["start", "api", "--no-daemon"]);
    assert_eq!(code, 6);
    assert_eq!(error_code(&value), "no_supervisor");
}

#[test]
fn concurrent_adds_are_all_kept() {
    let sandbox = Sandbox::new();
    let children: Vec<_> = (0..8)
        .map(|i| {
            let name = format!("App {i}");
            sandbox
                .command(&["add", "--name", &name, "--dir", ".", "--kind", "raw", "--command", "true"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    assert_eq!(sandbox.ok(&["list"])["apps"].as_array().unwrap().len(), 8);
}

#[test]
fn full_lifecycle_through_the_background_daemon() {
    let sandbox = Sandbox::new();
    let id = sandbox.add_raw("Ticker", "echo started; while true; do echo tick; sleep 0.2; done");
    sandbox.add_raw("Crasher", "echo boom; exit 3");

    let started = sandbox.ok(&["start", "ticker", "--wait", "running", "--timeout", "30"]);
    assert_eq!(started["spawned_daemon"], true);
    assert_eq!(started["supervisor"]["role"], "daemon");
    assert_eq!(started["results"][0]["outcome"], "started");
    assert_eq!(started["results"][0]["runtime"]["status"], "running");

    let again = sandbox.ok(&["start", "ticker"]);
    assert_eq!(again["spawned_daemon"], false);
    assert_eq!(again["results"][0]["outcome"], "already_running");

    let logs = wait_for_log(&sandbox, "ticker", "tick");
    assert!(logs["next_seq"].as_u64().unwrap() >= 2);
    let next = logs["next_seq"].as_u64().unwrap().to_string();
    let later = sandbox.ok(&["logs", "ticker", "--since", &next]);
    assert_eq!(later["first_seq"].as_u64().unwrap().to_string(), next);

    let edited = sandbox.ok(&["edit", "ticker", "--command", "echo v2; while true; do sleep 1; done"]);
    assert_eq!(edited["restart_required"], true);
    let restarted = sandbox.ok(&["restart", &id, "--wait", "running", "--timeout", "30"]);
    assert_eq!(restarted["results"][0]["outcome"], "restarting");
    wait_for_log(&sandbox, "ticker", "v2");

    let (code, failed) = sandbox.json(&["start", "crasher", "--wait", "running", "--timeout", "30"]);
    assert_eq!(code, 8, "{failed:#}");
    assert_eq!(error_code(&failed), "app_failed");

    let stopped = sandbox.ok(&["stop", "ticker"]);
    assert_eq!(stopped["results"][0]["runtime"]["status"], "stopped");

    let follow = sandbox.run(&["--json", "logs", "ticker", "--follow", "--timeout", "5"]);
    let events: Vec<Value> = String::from_utf8_lossy(&follow.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).expect("JSON line"))
        .collect();
    assert_eq!(events.last().unwrap()["type"], "end");
    assert_eq!(events.last().unwrap()["reason"], "stopped");
    assert!(events.iter().any(|e| e["type"] == "line" && e["text"] == "v2"));

    let shutdown = sandbox.ok(&["daemon", "stop"]);
    assert_eq!(shutdown["stopped"], true);
    assert_eq!(sandbox.ok(&["daemon", "status"])["supervisor"], Value::Null);
    assert!(!sandbox.config_dir().join("control.json").exists());
}

#[test]
fn daemon_run_refuses_to_start_twice() {
    let sandbox = Sandbox::new();
    sandbox.ok(&["daemon", "start"]);
    let (code, value) = sandbox.json(&["daemon", "run"]);
    assert_eq!(code, 6);
    assert_eq!(error_code(&value), "already_supervised");
}

fn wait_for_log(sandbox: &Sandbox, app: &str, needle: &str) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let logs = sandbox.ok(&["logs", app]);
        if logs["lines"].as_array().unwrap().iter().any(|l| l.as_str() == Some(needle)) {
            return logs;
        }
        assert!(std::time::Instant::now() < deadline, "\"{needle}\" never showed up in {logs:#}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
