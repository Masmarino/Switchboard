use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use switchboard_core::control::{ControlRequest, SupervisorRole, DEFAULT_REQUEST_TIMEOUT};
use switchboard_core::{DaemonStartError, Engine};
use uuid::Uuid;

use crate::context::{supervisor_json, supervisor_label, Context};
use crate::daemon_spawn::DAEMON_LOG_FILE;
use crate::error::{CliError, ErrorCode};
use crate::output::Output;

const TICK: Duration = Duration::from_millis(100);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(6);

/// Boucle du superviseur headless : meme moteur que les apps natives, sans fenetre.
pub fn run(ctx: &Context, idle_timeout: u64) -> Result<Output, CliError> {
    let mut engine = Engine::new_daemon(ctx.dir()).map_err(|e| match e {
        DaemonStartError::AlreadySupervised { .. } => CliError::new(ErrorCode::AlreadySupervised, e.to_string())
            .with_hint("CLI commands already go to it; nothing else to start"),
        DaemonStartError::Io(_) => CliError::new(ErrorCode::DaemonStartFailed, e.to_string()),
    })?;
    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let interrupted = interrupted.clone();
        let _ = ctrlc::set_handler(move || interrupted.store(true, Ordering::SeqCst));
    }
    eprintln!("switchboard daemon {} supervising {}", std::process::id(), ctx.config.path().display());

    let idle_limit = (idle_timeout > 0).then(|| Duration::from_secs(idle_timeout));
    let mut last_active = Instant::now();
    let reason = loop {
        engine.tick();
        if engine.shutdown_requested() {
            break "requested";
        }
        if interrupted.load(Ordering::SeqCst) {
            break "interrupted";
        }
        if engine.has_active_apps() {
            last_active = Instant::now();
        } else if idle_limit.is_some_and(|limit| last_active.elapsed() >= limit) {
            break "idle";
        }
        std::thread::sleep(TICK);
    };
    engine.shutdown(SHUTDOWN_GRACE);
    drop(engine);
    eprintln!("switchboard daemon {} exiting ({reason})", std::process::id());
    Ok(Output::new(json!({ "reason": reason }), format!("Daemon stopped ({reason})")))
}

pub fn start(ctx: &Context) -> Result<Output, CliError> {
    let (client, spawned) = ctx.supervisor_or_spawn(true)?;
    let human = if spawned {
        format!("Daemon started (pid {})", client.endpoint().pid)
    } else {
        format!("Already supervised by {}", supervisor_label(Some(&client)))
    };
    Ok(Output::new(json!({ "supervisor": supervisor_json(Some(&client)), "spawned_daemon": spawned }), human))
}

pub fn stop(ctx: &Context) -> Result<Output, CliError> {
    let Some(client) = ctx.supervisor() else {
        return Ok(Output::new(json!({ "stopped": false, "stopped_apps": [] }), "No daemon running"));
    };
    if client.endpoint().role != SupervisorRole::Daemon {
        return Err(CliError::new(ErrorCode::SupervisorError, "apps are supervised by the Switchboard app, not by a daemon")
            .with_hint("use `switchboard-cli stop --all` to stop the apps, or quit the app"));
    }
    let stopped: Vec<Uuid> = client.request_as(&ControlRequest::Shutdown, DEFAULT_REQUEST_TIMEOUT)?;
    // Le daemon libere son canal une fois ses process arretes.
    let deadline = Instant::now() + Duration::from_secs(15);
    while ctx.supervisor().is_some_and(|c| c.endpoint().pid == client.endpoint().pid) {
        if Instant::now() >= deadline {
            return Err(CliError::new(ErrorCode::Timeout, "the daemon is still shutting down after 15s"));
        }
        std::thread::sleep(TICK);
    }
    Ok(Output::new(
        json!({ "stopped": true, "pid": client.endpoint().pid, "stopped_apps": stopped }),
        format!("Daemon {} stopped ({} app(s) stopped)", client.endpoint().pid, stopped.len()),
    ))
}

pub fn status(ctx: &Context) -> Result<Output, CliError> {
    let client = ctx.supervisor();
    Ok(Output::new(
        json!({ "supervisor": supervisor_json(client.as_ref()), "daemon_log": ctx.dir().join(DAEMON_LOG_FILE).display().to_string() }),
        format!("supervisor: {}", supervisor_label(client.as_ref())),
    ))
}

pub fn info(ctx: &Context) -> Result<Output, CliError> {
    let client = ctx.supervisor();
    let path = ctx.config.path().display().to_string();
    Ok(Output::new(
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "config_dir": ctx.dir().display().to_string(),
            "config_path": path,
            "config_exists": ctx.config.path().exists(),
            "supervisor": supervisor_json(client.as_ref()),
            "daemon_log": ctx.dir().join(DAEMON_LOG_FILE).display().to_string(),
        }),
        format!(
            "switchboard-cli {}\nconfig:     {path}\nsupervisor: {}",
            env!("CARGO_PKG_VERSION"),
            supervisor_label(client.as_ref())
        ),
    ))
}

