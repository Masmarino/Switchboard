//! Canal local entre le CLI et le process qui supervise les apps (app native ou daemon).

pub mod client;
pub mod endpoint;
pub mod protocol;
pub mod server;
mod transport;

pub use client::{ControlError, SupervisorClient, DEFAULT_REQUEST_TIMEOUT};
pub use endpoint::{Endpoint, SupervisorRole};
pub use protocol::{ActionOutcome, ActionResult, ControlRequest, ControlResponse, LogChunk, RuntimeState, SupervisorInfo};
pub use server::ControlServer;
