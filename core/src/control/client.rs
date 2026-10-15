use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::control::endpoint::Endpoint;
use crate::control::protocol::{ControlEnvelope, ControlRequest, ControlResponse};
use crate::control::transport::{self, ControlStream};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub enum ControlError {
    Io(std::io::Error),
    Protocol(String),
    Remote { code: String, message: String },
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlError::Io(e) => write!(f, "cannot reach the supervisor: {e}"),
            ControlError::Protocol(msg) => write!(f, "unexpected supervisor answer: {msg}"),
            ControlError::Remote { code, message } => write!(f, "{message} ({code})"),
        }
    }
}

impl std::error::Error for ControlError {}

/// Client du superviseur (app native ou daemon) publie dans `control.json`.
#[derive(Debug, Clone)]
pub struct SupervisorClient {
    endpoint: Endpoint,
}

impl SupervisorClient {
    /// `None` si personne ne supervise : pas de `control.json`, ou fichier perime
    /// (process mort, socket qui refuse la connexion).
    pub fn discover(dir: &Path) -> Option<Self> {
        let client = Self { endpoint: Endpoint::read(dir)? };
        client.request(&ControlRequest::Ping, Duration::from_secs(5)).ok()?;
        Some(client)
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn request(&self, request: &ControlRequest, timeout: Duration) -> Result<serde_json::Value, ControlError> {
        let stream = transport::connect(&self.endpoint.address, CONNECT_TIMEOUT).map_err(ControlError::Io)?;
        stream.set_timeouts(timeout).map_err(ControlError::Io)?;
        let mut writer = stream.try_clone_stream().map_err(ControlError::Io)?;
        let envelope = ControlEnvelope { token: self.endpoint.token.clone(), request: request.clone() };
        let json = serde_json::to_string(&envelope).map_err(|e| ControlError::Protocol(e.to_string()))?;
        writeln!(writer, "{json}").and_then(|_| writer.flush()).map_err(ControlError::Io)?;

        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).map_err(ControlError::Io)?;
        match serde_json::from_str::<ControlResponse>(&line) {
            Ok(ControlResponse::Ok { data }) => Ok(data),
            Ok(ControlResponse::Error { code, message }) => Err(ControlError::Remote { code, message }),
            Err(e) if line.is_empty() => Err(ControlError::Protocol(format!("connection closed ({e})"))),
            Err(e) => Err(ControlError::Protocol(e.to_string())),
        }
    }

    pub fn request_as<T: DeserializeOwned>(&self, request: &ControlRequest, timeout: Duration) -> Result<T, ControlError> {
        let data = self.request(request, timeout)?;
        serde_json::from_value(data).map_err(|e| ControlError::Protocol(e.to_string()))
    }
}
