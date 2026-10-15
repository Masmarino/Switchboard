# switchboard-cli — guide for AI agents and scripts

`switchboard-cli` manages and controls Switchboard's launch apps (the dev servers listed in the
Switchboard window) without a GUI. It is built for agents such as Claude Code: every command has a
stable `--json` output, explicit exit codes, actionable error messages and never prompts.

## Install

```sh
make cli                       # cargo install --path cli --locked → ~/.cargo/bin/switchboard-cli
```

Release builds also ship `switchboard-cli-<version>-<os>-<arch>` archives (signed and notarized on
macOS when the signing secrets are set). The Linux tarball contains `bin/switchboard-cli`, and the
Windows installer puts `switchboard-cli.exe` next to the app and can add it to the `PATH`.

## How it fits together

- **Configuration** lives in `apps.json`, the same file the native apps use
  (`switchboard-cli info` prints its path; override with `--config-dir` or `SWITCHBOARD_CONFIG_DIR`).
  The CLI edits it directly, under a file lock, with an atomic write. Open Switchboard windows
  reload it within a second (macOS, Linux and Windows share this logic in `switchboard-core`).
- **Processes** are owned by a *supervisor*: the open Switchboard app if there is one, otherwise a
  headless daemon that `start`/`restart` launch in the background (`switchboard-cli daemon run`).
  Process commands (`start`, `stop`, `restart`, `status`, `logs`, `wait`) talk to it over a local
  channel (Unix socket, or loopback TCP with a token on Windows) described in `control.json`.
- When a Switchboard window opens while the daemon runs, the window takes over: the daemon stops
  its apps, hands their list over and exits, and the window starts them again. Quitting the window
  stops its apps and waits for them to exit, as before.
- The supervisor records its processes in `running.json`. If it is killed without stopping them,
  the next supervisor (window or daemon) stops the processes it left behind.
- **Logs** are kept in memory by the supervisor (last 5000 lines per app). They disappear when the
  supervisor exits. The daemon exits after 30 minutes without any running app
  (`--idle-timeout`, `SWITCHBOARD_DAEMON_IDLE_TIMEOUT`, 0 = never).

## Output contract

With `--json` (or `SWITCHBOARD_JSON=1`) every command prints exactly one JSON document on stdout:

```json
{"ok": true, "command": "add", "data": {...}, "warnings": [{"field": "working_dir", "code": "missing", "message": "..."}]}
{"ok": false, "command": "add", "error": {"code": "validation_failed", "message": "...", "hint": "...", "details": {...}}, "exit_code": 4}
```

Exception: `logs --follow --json` prints JSON Lines: `{"type":"line","seq":N,"text":"..."}`,
`{"type":"reset","first_seq":N}` when lines were dropped, and a final
`{"type":"end","reason":"stopped|timeout|interrupted","status":"...","next_seq":N}`.

| Exit code | Error codes | Meaning |
|---|---|---|
| 0 | | success |
| 1 | `io_error`, `internal` | file system or unexpected failure |
| 2 | `usage`, `invalid_input` | bad flags, malformed JSON / `KEY=VALUE` / import file |
| 3 | `app_not_found`, `ambiguous_app` | the app reference matches nothing / several apps |
| 4 | `validation_failed` | the app breaks a rule, `error.details.errors` lists each `{field, code, message}` |
| 5 | `config_invalid` | `apps.json` is not valid JSON; the CLI never overwrites it |
| 6 | `no_supervisor`, `already_supervised`, `supervisor_error`, `daemon_start_failed` | process control unavailable |
| 7 | `timeout` | `wait`/`--wait` did not reach the state in time |
| 8 | `app_failed` | the app crashed or exited while waiting for it |

An app in `data` looks like this (keys are sorted alphabetically in the output):

```json
{
  "id": "4d35942a-d53e-4d93-a286-23f50e85ae6b",
  "name": "Api",
  "kind": "npm",
  "working_dir": "/Users/me/code/api",
  "command": "run dev",
  "url": "http://localhost:3000",
  "env_vars": [["PORT", "3000"]],
  "auto_restart": false,
  "start_order": 0,
  "launch": ["npm run dev"],
  "runtime": {"status": "running", "active": true, "error": null, "healthy": true,
              "cpu_percent": 1.5, "memory_mb": 80.2, "restarting": false, "queued": false}
}
```

`runtime.status` is `stopped`, `building` (cargo build), `running` or `failed` (see `runtime.error`).

## App schema

`switchboard-cli schema --json` is the reference: fields, types, rules, defaults, kinds, statuses,
exit and error codes. In short:

| Field | Flag | Rules |
|---|---|---|
| `name` | `--name` | required, unique (case-insensitive) |
| `working_dir` | `--dir` | required, absolute (relative paths and `~` are resolved by the CLI); warning if missing |
| `kind` | `--kind` | `cargo`, `npm`, `dotnet`, `maven`, `python`, `go`, `raw` |
| `command` | `--command` | required for `npm` and `raw`, ignored for `cargo`, optional otherwise; no unclosed quote |
| `url` | `--url`, `--no-url` | `http(s)://…`; enables health checks |
| `env_vars` | `--env K=V`, `--unset-env K`, `--clear-env` | keys without spaces or `=`, no duplicates |
| `auto_restart` | `--auto-restart [true\|false]` | relaunch after a crash |
| `start_order` | `--start-order N` | 0–99, lower tiers start first with `start --all` |

Apps are referenced by id, unique id prefix (4+ characters) or name (case-insensitive).
`add` and `edit` also accept `--from-json '<object>'` (`-` = stdin, `@file` = file) with the same
field names; flags override it. `env_vars` may be `[["K","V"]]` or `{"K":"V"}`.

## Recipes

```sh
# Discover
switchboard-cli schema --json
switchboard-cli list --json

# Create an app for the current project and run it until it answers
switchboard-cli add --json --name "Alume API" --dir . --kind cargo --url http://localhost:3000/health \
  --env RUST_LOG=info --auto-restart
switchboard-cli start "alume api" --wait healthy --timeout 300 --json

# Same thing from JSON
echo '{"name":"Website","working_dir":"./website","kind":"npm","command":"run dev","url":"http://localhost:4200"}' \
  | switchboard-cli add --from-json - --json

# Inspect a failure
switchboard-cli status --json
switchboard-cli logs website --tail 100 --json

# Change something, then apply it (edit reports restart_required when the app runs)
switchboard-cli edit website --command "run start" --env PORT=4300 --json
switchboard-cli restart website --wait running --json

# Read only the new lines since the last call
switchboard-cli logs website --since 120 --json   # use data.next_seq from the previous call

# Follow until the app stops or 30 s pass
switchboard-cli logs website --follow --timeout 30 --json

# Stop (waits for the processes to exit; SIGKILL after 5 s)
switchboard-cli stop --all --json
switchboard-cli daemon stop --json

# Share or restore a set of apps (same format as the native export/import)
switchboard-cli export --include-env -o apps-backup.json
switchboard-cli import apps-backup.json --dry-run --json
```

## Tips for agents

- Always pass `--json` and branch on `ok` / `error.code`, not on the message text.
- `start` returns as soon as the processes are launched. Add `--wait running` (process alive for
  2 s) or `--wait healthy` (its `url` answers; apps without url behave like `running`).
- `stop` waits for the processes to exit by default; `start` right after `stop` is safe.
- Editing an app never restarts it: check `data.restart_required` and call `restart`.
- `start` launches a background daemon when no Switchboard window is open (`data.spawned_daemon`).
  Run `switchboard-cli daemon stop` when you are done if you do not want the apps to keep running.
- Env var values are printed in clear by `list`/`show` and written to `apps.json` (mode 0600).
  `export` leaves them out unless `--include-env` is given.
- `command` is split like a shell: `--command '-c "import app; app.run()"'` keeps the quoted part
  as one argument. `raw` commands go to `sh -c` unchanged.
- Commands run from `working_dir` in a login, interactive shell (`$SHELL -i -l -c`; `cmd.exe /C` on
  Windows), like the native apps, so `cargo`, `npm`, `nvm`… are found as in a terminal.
