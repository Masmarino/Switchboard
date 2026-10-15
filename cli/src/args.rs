use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

const AFTER_HELP: &str = "\
APP REFERENCES:
  <APP> is an app id, a unique id prefix (4+ characters) or its name (case-insensitive).

OUTPUT:
  --json prints one JSON document on stdout for every command, success or failure:
    {\"ok\": true,  \"command\": \"...\", \"data\": {...}, \"warnings\": [...]}
    {\"ok\": false, \"command\": \"...\", \"error\": {\"code\", \"message\", \"hint\", \"details\"}, \"exit_code\": N}
  `logs --follow --json` prints JSON Lines instead (one event per line).

EXIT CODES:
  0 ok · 1 io/internal · 2 usage/invalid input · 3 app not found/ambiguous · 4 validation failed
  5 config file invalid · 6 supervisor unavailable/refused · 7 timeout · 8 app failed/exited

Run `switchboard-cli schema --json` for the app fields, kinds and error codes.";

/// Manage and control Switchboard launch apps from scripts and AI agents.
///
/// Edits the same apps.json as the native apps (which reload it within a second). Process
/// commands (start/stop/restart/status/logs/wait) go to whoever supervises the apps: the
/// open Switchboard app if any, otherwise a background daemon started on demand.
#[derive(Debug, Parser)]
#[command(name = "switchboard-cli", version, after_help = AFTER_HELP, propagate_version = true)]
pub struct Cli {
    /// Machine-readable output (stable JSON envelope on stdout).
    #[arg(long, global = true, env = "SWITCHBOARD_JSON", value_parser = clap::builder::FalseyValueParser::new())]
    pub json: bool,

    /// Directory holding apps.json (default: the native apps' config directory).
    #[arg(long, global = true, value_name = "DIR", env = "SWITCHBOARD_CONFIG_DIR")]
    pub config_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List apps with their status.
    List,
    /// Show one app: configuration, launch commands and status.
    Show {
        /// App id, id prefix or name.
        app: String,
    },
    /// Add an app (validated like the native form; prints the created app).
    Add(AddArgs),
    /// Change some fields of an app; other fields are kept.
    Edit(EditArgs),
    /// Remove apps (stopping them first if they run).
    #[command(visible_alias = "rm")]
    Remove {
        /// App ids, id prefixes or names.
        #[arg(required = true)]
        apps: Vec<String>,
    },
    /// Export apps in the native apps' import format.
    Export(ExportArgs),
    /// Merge a config file into apps.json (same rules as the native import: matched by name).
    Import(ImportArgs),
    /// Describe app fields, kinds, statuses, exit codes and error codes.
    Schema,
    /// Version, config path and current supervisor.
    Info,
    /// Start apps (starts a background daemon if no Switchboard window is open).
    Start(StartArgs),
    /// Stop apps and wait until their processes have exited.
    Stop(StopArgs),
    /// Restart apps with their current configuration.
    Restart(RestartArgs),
    /// Runtime status of apps (all apps if none given).
    Status {
        /// App ids, id prefixes or names.
        apps: Vec<String>,
    },
    /// Print an app's captured output (kept in memory by the supervisor, last 5000 lines).
    Logs(LogsArgs),
    /// Wait until apps reach a state.
    Wait(WaitArgs),
    /// Run or control the headless supervisor used when no Switchboard window is open.
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
}

/// Fields shared by `add` and `edit`; omitted flags leave the field unchanged (edit) or default (add).
#[derive(Debug, Args, Default)]
pub struct AppFields {
    /// Display name, unique (case-insensitive).
    #[arg(long)]
    pub name: Option<String>,
    /// Working directory; relative paths are resolved against the current directory.
    #[arg(long, value_name = "PATH")]
    pub dir: Option<PathBuf>,
    /// cargo, npm, dotnet, maven, python, go or raw.
    #[arg(long)]
    pub kind: Option<String>,
    /// Arguments for the kind's program (raw: a full shell command).
    #[arg(long, allow_hyphen_values = true)]
    pub command: Option<String>,
    /// Local URL (http/https), used for health checks and "open in browser".
    #[arg(long)]
    pub url: Option<String>,
    /// Environment variable KEY=VALUE (repeatable; replaces an existing KEY).
    #[arg(long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// Restart automatically after a crash.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    pub auto_restart: Option<bool>,
    /// Tier for `start --all`: lower starts first, equal values start together (0-99).
    #[arg(long, value_name = "N", allow_negative_numbers = true)]
    pub start_order: Option<i32>,
    /// JSON object with any of the app fields (see `schema`); `-` reads stdin, `@file` reads a file.
    /// Flags given alongside override it.
    #[arg(long, value_name = "JSON")]
    pub from_json: Option<String>,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    #[command(flatten)]
    pub fields: AppFields,
}

#[derive(Debug, Args)]
pub struct EditArgs {
    /// App id, id prefix or name.
    pub app: String,
    #[command(flatten)]
    pub fields: AppFields,
    /// Remove the URL (disables health checks).
    #[arg(long, conflicts_with = "url")]
    pub no_url: bool,
    /// Remove an environment variable (repeatable).
    #[arg(long, value_name = "KEY")]
    pub unset_env: Vec<String>,
    /// Remove every environment variable before applying --env.
    #[arg(long)]
    pub clear_env: bool,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Apps to export (all apps if none given).
    pub apps: Vec<String>,
    /// Keep environment variables (they may hold secrets; excluded by default).
    #[arg(long)]
    pub include_env: bool,
    /// Write to this file instead of stdout.
    #[arg(long, short, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// Config file to import, or `-` for stdin.
    pub file: String,
    /// Report what would be added/replaced without writing.
    #[arg(long)]
    pub dry_run: bool,
    /// Accept apps the CLI rules would reject (the native import only rejects blank names).
    #[arg(long)]
    pub skip_validation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WaitTarget {
    /// The process is up and has not exited for 2 s (cargo: after the build).
    Running,
    /// Its url answers with a status below 500 (apps without url: same as running).
    Healthy,
    /// The process has exited.
    Stopped,
}

#[derive(Debug, Args)]
pub struct StartArgs {
    /// Apps to start.
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    pub apps: Vec<String>,
    /// Start every app, tier by tier (start_order).
    #[arg(long)]
    pub all: bool,
    /// Fail with no_supervisor instead of starting a background daemon.
    #[arg(long)]
    pub no_daemon: bool,
    /// Block until the apps are running or healthy.
    #[arg(long, value_enum, value_name = "STATE")]
    pub wait: Option<WaitTarget>,
    /// Seconds to wait with --wait.
    #[arg(long, default_value_t = 120, value_name = "SECS")]
    pub timeout: u64,
}

#[derive(Debug, Args)]
pub struct StopArgs {
    /// Apps to stop.
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    pub apps: Vec<String>,
    /// Stop every running app.
    #[arg(long)]
    pub all: bool,
    /// Return as soon as the stop signal is sent.
    #[arg(long)]
    pub no_wait: bool,
    /// Seconds to wait for the processes to exit (SIGKILL is sent after 5 s).
    #[arg(long, default_value_t = 15, value_name = "SECS")]
    pub timeout: u64,
}

#[derive(Debug, Args)]
pub struct RestartArgs {
    /// Apps to restart (stopped apps are started).
    #[arg(required = true)]
    pub apps: Vec<String>,
    /// Fail with no_supervisor instead of starting a background daemon.
    #[arg(long)]
    pub no_daemon: bool,
    /// Block until the apps are running or healthy again.
    #[arg(long, value_enum, value_name = "STATE")]
    pub wait: Option<WaitTarget>,
    /// Seconds to wait with --wait.
    #[arg(long, default_value_t = 120, value_name = "SECS")]
    pub timeout: u64,
}

#[derive(Debug, Args)]
pub struct LogsArgs {
    /// App id, id prefix or name.
    pub app: String,
    /// Only the last N lines.
    #[arg(long, short = 'n', value_name = "N")]
    pub tail: Option<usize>,
    /// Only lines after this sequence number (use `next_seq` from a previous call).
    #[arg(long, value_name = "SEQ", conflicts_with = "tail")]
    pub since: Option<u64>,
    /// Keep printing new lines until the app stops, --timeout elapses or Ctrl-C.
    #[arg(long, short)]
    pub follow: bool,
    /// Stop following after this many seconds.
    #[arg(long, value_name = "SECS", requires = "follow")]
    pub timeout: Option<u64>,
}

#[derive(Debug, Args)]
pub struct WaitArgs {
    /// Apps to wait for.
    #[arg(required = true)]
    pub apps: Vec<String>,
    /// State to wait for.
    #[arg(long = "for", value_enum, value_name = "STATE")]
    pub target: WaitTarget,
    /// Seconds before giving up (exit code 7).
    #[arg(long, default_value_t = 60, value_name = "SECS")]
    pub timeout: u64,
}

#[derive(Debug, Subcommand)]
pub enum DaemonAction {
    /// Run the supervisor in the foreground (what `start` launches in the background).
    Run {
        /// Exit after this many seconds without any running app (0 = never).
        #[arg(long, default_value_t = 1800, value_name = "SECS", env = "SWITCHBOARD_DAEMON_IDLE_TIMEOUT")]
        idle_timeout: u64,
    },
    /// Start the background daemon if nothing supervises the apps yet.
    Start,
    /// Stop the background daemon and every app it runs.
    Stop,
    /// Show who supervises the apps (native app, daemon or nobody).
    Status,
}
