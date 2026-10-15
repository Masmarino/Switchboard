use std::sync::mpsc;
use uuid::Uuid;

use crate::control::protocol::{ControlRequest, ControlResponse};
use crate::process_manager::AppStatus;

#[derive(Debug, Clone)]
pub enum Event {
    Log(Uuid, String),
    StatusChanged(Uuid, AppStatus),
    HealthChanged(Uuid, bool),
    StartRequested(Uuid),
    Control(ControlRequest, mpsc::Sender<ControlResponse>),
}

pub fn stream_lines<R: std::io::Read>(reader: R, app_id: Uuid, tx: std::sync::mpsc::Sender<Event>) {
    use std::io::BufRead;
    let buf = std::io::BufReader::new(reader);
    for line in buf.lines() {
        match line {
            Ok(text) => {
                if tx.send(Event::Log(app_id, text)).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}
