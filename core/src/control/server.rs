use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use crate::control::endpoint::{Endpoint, SupervisorRole};
use crate::control::protocol::{ControlEnvelope, ControlResponse};
use crate::control::transport::{self, ControlStream};
use crate::log_stream::Event;

const MAX_REQUEST_BYTES: u64 = 64 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Large : un `handover` attend l'arret effectif des process (SIGKILL compris).
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// Ecoute les requetes du CLI et les transmet au moteur via son canal d'evenements :
/// elles sont traitees au prochain `poll` du frontend (toutes les 200 ms).
pub struct ControlServer {
    dir: PathBuf,
    endpoint: Endpoint,
    shutdown: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
}

impl ControlServer {
    pub(crate) fn start(dir: &Path, role: SupervisorRole, tx: mpsc::Sender<Event>) -> std::io::Result<Self> {
        let (listener, address) = transport::bind(dir)?;
        let endpoint = Endpoint {
            pid: std::process::id(),
            role,
            version: env!("CARGO_PKG_VERSION").to_string(),
            address,
            token: uuid::Uuid::new_v4().simple().to_string(),
        };
        if let Err(e) = endpoint.write(dir) {
            transport::cleanup(&endpoint.address);
            return Err(e);
        }
        let shutdown = Arc::new(AtomicBool::new(false));
        let in_flight = Arc::new(AtomicUsize::new(0));
        {
            let shutdown = shutdown.clone();
            let in_flight = in_flight.clone();
            let token = endpoint.token.clone();
            std::thread::spawn(move || accept_loop(listener, token, tx, shutdown, in_flight));
        }
        Ok(Self { dir: dir.to_path_buf(), endpoint, shutdown, in_flight })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Connexions en cours de traitement ; survit au serveur pour qu'un process qui se termine
    /// puisse attendre que ses dernieres reponses soient ecrites.
    pub fn in_flight(&self) -> Arc<AtomicUsize> {
        self.in_flight.clone()
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Debloque `accept` avant de supprimer la socket, sinon le thread reste bloque.
        let _ = transport::connect(&self.endpoint.address, Duration::from_millis(200));
        // Ne supprime que ce qui nous appartient encore : apres un handover, l'app native
        // a peut-etre deja publie sa propre socket au meme endroit.
        if Endpoint::read(&self.dir).is_some_and(|current| current.pid == self.endpoint.pid && current.token == self.endpoint.token) {
            let _ = std::fs::remove_file(Endpoint::path_in(&self.dir));
            transport::cleanup(&self.endpoint.address);
        }
    }
}

fn accept_loop(
    listener: transport::Listener,
    token: String,
    tx: mpsc::Sender<Event>,
    shutdown: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
) {
    loop {
        let stream = transport::accept(&listener);
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = stream else { continue };
        let token = token.clone();
        let tx = tx.clone();
        let in_flight = in_flight.clone();
        in_flight.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let _ = serve_connection(stream, &token, &tx);
            in_flight.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

fn serve_connection<S: ControlStream>(stream: S, token: &str, tx: &mpsc::Sender<Event>) -> std::io::Result<()> {
    stream.set_timeouts(IO_TIMEOUT)?;
    let mut writer = stream.try_clone_stream()?;
    let mut reader = BufReader::new(stream.take(MAX_REQUEST_BYTES));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim().is_empty() {
        return Ok(());
    }
    let response = match serde_json::from_str::<ControlEnvelope>(&line) {
        Err(e) => ControlResponse::error("bad_request", format!("invalid control request: {e}")),
        Ok(envelope) if !constant_time_eq(envelope.token.as_bytes(), token.as_bytes()) => {
            ControlResponse::error("unauthorized", "invalid control token")
        }
        Ok(envelope) => dispatch(envelope, tx),
    };
    let json = serde_json::to_string(&response).map_err(std::io::Error::other)?;
    writeln!(writer, "{json}")?;
    writer.flush()
}

fn dispatch(envelope: ControlEnvelope, tx: &mpsc::Sender<Event>) -> ControlResponse {
    let (reply_tx, reply_rx) = mpsc::channel();
    if tx.send(Event::Control(envelope.request, reply_tx)).is_err() {
        return ControlResponse::error("shutting_down", "the supervisor is shutting down");
    }
    reply_rx.recv_timeout(REPLY_TIMEOUT).unwrap_or_else(|_| {
        ControlResponse::error("timeout", "the supervisor did not answer in time (is the app frozen?)")
    })
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_compares_contents_and_length() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
