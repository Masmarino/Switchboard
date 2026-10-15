use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use uuid::Uuid;

use crate::log_stream::Event;

const CHECK_INTERVAL: Duration = Duration::from_secs(5);
/// Tant que l'app ne repond pas (demarrage), on reessaie plus souvent.
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Ping periodique de l'URL d'une app tant qu'elle tourne, sur son propre thread —
/// distingue "le process tourne" de "l'app repond vraiment".
pub fn spawn_health_watcher(id: Uuid, url: String, stop_requested: Arc<AtomicBool>, tx: mpsc::Sender<Event>) {
    std::thread::spawn(move || {
        // Laisse le process le temps de demarrer avant le premier ping.
        std::thread::sleep(Duration::from_secs(1));
        let agent = ureq::Agent::config_builder().timeout_global(Some(REQUEST_TIMEOUT)).build().new_agent();
        while !stop_requested.load(Ordering::SeqCst) {
            let healthy = ping(&agent, &url);
            if tx.send(Event::HealthChanged(id, healthy)).is_err() {
                return;
            }
            std::thread::sleep(if healthy { CHECK_INTERVAL } else { RETRY_INTERVAL });
        }
    });
}

fn ping(agent: &ureq::Agent, url: &str) -> bool {
    agent.get(url).call().map(|resp| resp.status().as_u16() < 500).unwrap_or(false)
}
