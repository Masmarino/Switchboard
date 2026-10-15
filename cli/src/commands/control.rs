use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use switchboard_core::control::{
    ActionOutcome, ActionResult, ControlRequest, LogChunk, RuntimeState, SupervisorClient, DEFAULT_REQUEST_TIMEOUT,
};
use switchboard_core::{AppConfig, AppConfigList};
use uuid::Uuid;

use crate::app_json::{runtime_json, status_of};
use crate::app_ref::{resolve, resolve_many};
use crate::args::{LogsArgs, RestartArgs, StartArgs, StopArgs, WaitArgs, WaitTarget};
use crate::context::{supervisor_json, supervisor_label, Context};
use crate::error::{CliError, ErrorCode};
use crate::output::{table, Output};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// "running" n'est atteint qu'apres ce delai sans crash : un process qui meurt aussitot
/// lance (mauvaise commande, port pris) doit faire echouer `--wait running`.
const RUNNING_SETTLE: Duration = Duration::from_secs(2);

pub fn start(ctx: &Context, args: &StartArgs) -> Result<Output, CliError> {
    let config = ctx.load()?;
    let targets = if args.all { config.apps.clone() } else { resolve_many(&config, &args.apps)? };
    if targets.is_empty() {
        return Ok(Output::new(json!({ "spawned_daemon": false, "results": [] }), "No apps to start"));
    }
    let (client, spawned) = ctx.supervisor_or_spawn(!args.no_daemon)?;
    let results: Vec<ActionResult> = if args.all {
        let queued: Vec<Uuid> = client.request_as(&ControlRequest::StartAll, DEFAULT_REQUEST_TIMEOUT)?;
        targets
            .iter()
            .map(|a| ActionResult {
                id: a.id,
                outcome: if queued.contains(&a.id) { ActionOutcome::Started } else { ActionOutcome::AlreadyRunning },
                message: None,
            })
            .collect()
    } else {
        let ids = targets.iter().map(|a| a.id).collect();
        client.request_as(&ControlRequest::Start { ids }, DEFAULT_REQUEST_TIMEOUT)?
    };
    finish_action(&client, spawned, &config, results, args.wait, args.timeout)
}

pub fn restart(ctx: &Context, args: &RestartArgs) -> Result<Output, CliError> {
    let config = ctx.load()?;
    let targets = resolve_many(&config, &args.apps)?;
    let (client, spawned) = ctx.supervisor_or_spawn(!args.no_daemon)?;
    let ids = targets.iter().map(|a| a.id).collect();
    let results: Vec<ActionResult> = client.request_as(&ControlRequest::Restart { ids }, DEFAULT_REQUEST_TIMEOUT)?;
    finish_action(&client, spawned, &config, results, args.wait, args.timeout)
}

pub fn stop(ctx: &Context, args: &StopArgs) -> Result<Output, CliError> {
    let config = ctx.load()?;
    let targets = if args.all { config.apps.clone() } else { resolve_many(&config, &args.apps)? };
    let Some(client) = ctx.supervisor() else {
        // Personne ne supervise : rien ne peut tourner.
        let results: Vec<ActionResult> =
            targets.iter().map(|a| ActionResult { id: a.id, outcome: ActionOutcome::NotRunning, message: None }).collect();
        return Ok(action_output(&config, None, false, &results, None));
    };
    let ids: Vec<Uuid> = targets.iter().map(|a| a.id).collect();
    let results: Vec<ActionResult> = client.request_as(&ControlRequest::Stop { ids: ids.clone() }, DEFAULT_REQUEST_TIMEOUT)?;
    let final_states = if args.no_wait {
        None
    } else {
        let stopping: Vec<Uuid> =
            results.iter().filter(|r| r.outcome == ActionOutcome::Stopping).map(|r| r.id).collect();
        Some(wait_for(&client, &config, &stopping, WaitTarget::Stopped, Duration::from_secs(args.timeout))?)
    };
    Ok(action_output(&config, Some(&client), false, &results, final_states.as_ref()))
}

pub fn status(ctx: &Context, references: &[String]) -> Result<Output, CliError> {
    let config = ctx.load()?;
    let targets = if references.is_empty() { config.apps.clone() } else { resolve_many(&config, references)? };
    let (supervisor, runtimes) = ctx.runtimes()?;
    let apps: Vec<Value> = targets
        .iter()
        .map(|a| json!({ "id": a.id, "name": a.name, "runtime": runtime_json(runtimes.get(&a.id)) }))
        .collect();
    let rows: Vec<Vec<String>> = targets
        .iter()
        .map(|a| {
            let rt = runtimes.get(&a.id);
            vec![
                a.name.clone(),
                status_of(rt),
                rt.and_then(|r| r.healthy).map(|h| if h { "yes" } else { "no" }).unwrap_or("-").to_string(),
                rt.filter(|r| r.active).map(|r| format!("{:.0}%", r.cpu_percent)).unwrap_or_else(|| "-".to_string()),
                rt.filter(|r| r.active).map(|r| format!("{:.0} MB", r.memory_mb)).unwrap_or_else(|| "-".to_string()),
                rt.and_then(|r| r.error.clone()).unwrap_or_default(),
            ]
        })
        .collect();
    let human = format!(
        "{}\n\nsupervisor: {}",
        table(&["NAME", "STATUS", "HEALTHY", "CPU", "MEMORY", "ERROR"], &rows),
        supervisor_label(supervisor.as_ref())
    );
    Ok(Output::new(json!({ "supervisor": supervisor_json(supervisor.as_ref()), "apps": apps }), human))
}

pub fn logs(ctx: &Context, args: &LogsArgs) -> Result<Option<Output>, CliError> {
    let config = ctx.load()?;
    let app = resolve(&config, &args.app)?.clone();
    let client = ctx.require_supervisor().map_err(|e| {
        e.with_hint("logs live in the memory of the app window or daemon that ran the app; start it first")
    })?;
    let mut chunk: LogChunk =
        client.request_as(&ControlRequest::Logs { id: app.id, since: args.since.unwrap_or(0) }, DEFAULT_REQUEST_TIMEOUT)?;
    if let Some(tail) = args.tail {
        let skip = chunk.lines.len().saturating_sub(tail);
        chunk.lines.drain(..skip);
        chunk.first_seq += skip as u64;
    }
    if !args.follow {
        let human = chunk.lines.join("\n");
        return Ok(Some(Output::new(log_json(&app, &chunk), human)));
    }
    follow(ctx, &client, &app, chunk, args.timeout.map(Duration::from_secs))?;
    Ok(None)
}

/// Mode suivi : JSON Lines en `--json` (`{"type":"line"...}` puis un `{"type":"end"...}` final).
fn follow(ctx: &Context, client: &SupervisorClient, app: &AppConfig, first: LogChunk, timeout: Option<Duration>) -> Result<(), CliError> {
    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let interrupted = interrupted.clone();
        let _ = ctrlc::set_handler(move || interrupted.store(true, Ordering::SeqCst));
    }
    let deadline = timeout.map(|t| Instant::now() + t);
    let mut stdout = std::io::stdout();
    let mut chunk = first;
    let reason = loop {
        if chunk.replaced && ctx.json {
            emit(&mut stdout, &json!({ "type": "reset", "first_seq": chunk.first_seq }));
        }
        for (offset, line) in chunk.lines.iter().enumerate() {
            if ctx.json {
                emit(&mut stdout, &json!({ "type": "line", "seq": chunk.first_seq + offset as u64, "text": line }));
            } else {
                let _ = writeln!(stdout, "{line}");
            }
        }
        let _ = stdout.flush();
        if !chunk.active {
            break "stopped";
        }
        if interrupted.load(Ordering::SeqCst) {
            break "interrupted";
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break "timeout";
        }
        std::thread::sleep(POLL_INTERVAL);
        chunk = client.request_as(&ControlRequest::Logs { id: app.id, since: chunk.next_seq }, DEFAULT_REQUEST_TIMEOUT)?;
    };
    if ctx.json {
        emit(&mut stdout, &json!({ "type": "end", "reason": reason, "status": chunk.status, "next_seq": chunk.next_seq }));
    } else {
        eprintln!("[{}: {}]", app.name, if reason == "stopped" { chunk.status.as_str() } else { reason });
    }
    Ok(())
}

pub fn wait(ctx: &Context, args: &WaitArgs) -> Result<Output, CliError> {
    let config = ctx.load()?;
    let targets = resolve_many(&config, &args.apps)?;
    let ids: Vec<Uuid> = targets.iter().map(|a| a.id).collect();
    let Some(client) = ctx.supervisor() else {
        if args.target == WaitTarget::Stopped {
            return Ok(Output::new(json!({ "supervisor": null, "apps": [] }), "Nothing is running"));
        }
        return Err(ctx.require_supervisor().unwrap_err());
    };
    let states = wait_for(&client, &config, &ids, args.target, Duration::from_secs(args.timeout))?;
    let apps: Vec<Value> = targets
        .iter()
        .map(|a| json!({ "id": a.id, "name": a.name, "runtime": runtime_json(states.get(&a.id)) }))
        .collect();
    let names: Vec<&str> = targets.iter().map(|a| a.name.as_str()).collect();
    Ok(Output::new(
        json!({ "supervisor": supervisor_json(Some(&client)), "apps": apps }),
        format!("{} {}", names.join(", "), target_name(args.target)),
    ))
}

/// Interroge le superviseur jusqu'a ce que chaque app atteigne `target` ; echoue tot si une
/// app attendue "running"/"healthy" crashe ou se termine.
fn wait_for(
    client: &SupervisorClient,
    config: &AppConfigList,
    ids: &[Uuid],
    target: WaitTarget,
    timeout: Duration,
) -> Result<HashMap<Uuid, RuntimeState>, CliError> {
    let deadline = Instant::now() + timeout;
    let mut running_since: HashMap<Uuid, Instant> = HashMap::new();
    loop {
        let states: Vec<RuntimeState> = client.request_as(&ControlRequest::Status, DEFAULT_REQUEST_TIMEOUT)?;
        let states: HashMap<Uuid, RuntimeState> = states.into_iter().map(|s| (s.id, s)).collect();
        let mut pending = Vec::new();
        for id in ids {
            let Some(state) = states.get(id) else {
                return Err(CliError::new(ErrorCode::AppNotFound, format!("app {id} disappeared while waiting")));
            };
            let app = config.find(*id);
            let name = app.map(|a| a.name.as_str()).unwrap_or("?");
            let has_url = app.is_some_and(|a| a.url.is_some());
            // Ancien process encore en vie (restart) ou palier de `start --all` pas encore atteint.
            let transitioning = state.restarting || state.queued;
            let settled = if state.status == "running" && !transitioning {
                running_since.entry(*id).or_insert_with(Instant::now).elapsed() >= RUNNING_SETTLE
            } else {
                running_since.remove(id);
                false
            };
            let reached = !transitioning
                && match target {
                    WaitTarget::Stopped => !state.active,
                    WaitTarget::Running => settled,
                    WaitTarget::Healthy if has_url => state.status == "running" && state.healthy == Some(true),
                    WaitTarget::Healthy => settled,
                };
            if target != WaitTarget::Stopped && !transitioning && !state.active {
                let message = match &state.error {
                    Some(error) => format!("{name} failed: {error}"),
                    None => format!("{name} exited ({})", state.status),
                };
                return Err(CliError::new(ErrorCode::AppFailed, message)
                    .with_hint(format!("see `switchboard-cli logs {id} --tail 50`"))
                    .with_details(json!({ "id": id, "name": name, "runtime": runtime_json(Some(state)) })));
            }
            if !reached {
                pending.push(json!({ "id": id, "name": name, "runtime": runtime_json(Some(state)) }));
            }
        }
        if pending.is_empty() {
            return Ok(states);
        }
        if Instant::now() >= deadline {
            let names: Vec<&str> = pending.iter().filter_map(|p| p["name"].as_str()).collect();
            return Err(CliError::new(
                ErrorCode::Timeout,
                format!("timed out after {}s waiting for {} to be {}", timeout.as_secs(), names.join(", "), target_name(target)),
            )
            .with_details(json!({ "pending": pending })));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn finish_action(
    client: &SupervisorClient,
    spawned: bool,
    config: &AppConfigList,
    results: Vec<ActionResult>,
    wait: Option<WaitTarget>,
    timeout: u64,
) -> Result<Output, CliError> {
    if let Some(failed) = results.iter().find(|r| r.outcome == ActionOutcome::Failed) {
        let name = config.find(failed.id).map(|a| a.name.as_str()).unwrap_or("?");
        return Err(CliError::new(
            ErrorCode::AppFailed,
            format!("{name} could not start: {}", failed.message.as_deref().unwrap_or("unknown error")),
        )
        .with_details(json!({ "results": results })));
    }
    let final_states = match wait {
        Some(target) => {
            let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
            Some(wait_for(client, config, &ids, target, Duration::from_secs(timeout))?)
        }
        None => None,
    };
    Ok(action_output(config, Some(client), spawned, &results, final_states.as_ref()))
}

fn action_output(
    config: &AppConfigList,
    client: Option<&SupervisorClient>,
    spawned: bool,
    results: &[ActionResult],
    final_states: Option<&HashMap<Uuid, RuntimeState>>,
) -> Output {
    let entries: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "name": config.find(r.id).map(|a| a.name.as_str()),
                "outcome": r.outcome,
                "message": r.message,
                "runtime": final_states.and_then(|s| s.get(&r.id)).map(|s| runtime_json(Some(s))),
            })
        })
        .collect();
    let mut lines: Vec<String> = results
        .iter()
        .map(|r| {
            let name = config.find(r.id).map(|a| a.name.as_str()).unwrap_or("?");
            format!("{name}: {}", outcome_label(r.outcome))
        })
        .collect();
    if spawned {
        lines.push("(started the background daemon: `switchboard-cli daemon stop` stops it and its apps)".to_string());
    }
    Output::new(
        json!({ "supervisor": supervisor_json(client), "spawned_daemon": spawned, "results": entries }),
        lines.join("\n"),
    )
}

fn log_json(app: &AppConfig, chunk: &LogChunk) -> Value {
    json!({
        "id": app.id,
        "name": app.name,
        "status": chunk.status,
        "active": chunk.active,
        "lines": chunk.lines,
        "first_seq": chunk.first_seq,
        "next_seq": chunk.next_seq,
        "replaced": chunk.replaced,
    })
}

fn emit(stdout: &mut std::io::Stdout, value: &Value) {
    let _ = writeln!(stdout, "{value}");
}

fn outcome_label(outcome: ActionOutcome) -> &'static str {
    match outcome {
        ActionOutcome::Started => "started",
        ActionOutcome::AlreadyRunning => "already running",
        ActionOutcome::Stopping => "stopped",
        ActionOutcome::NotRunning => "not running",
        ActionOutcome::Restarting => "restarting",
        ActionOutcome::NotFound => "not found",
        ActionOutcome::Failed => "failed",
    }
}

fn target_name(target: WaitTarget) -> &'static str {
    match target {
        WaitTarget::Running => "running",
        WaitTarget::Healthy => "healthy",
        WaitTarget::Stopped => "stopped",
    }
}
