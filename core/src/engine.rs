use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::app_config::{AppConfigList, AppKind, ImportSummary};
use crate::config_error::ConfigError;
use crate::config_file::{ConfigFile, FileSignature};
use crate::control::{ActionOutcome, ControlRequest, ControlServer, SupervisorClient, SupervisorRole};
use crate::health::spawn_health_watcher;
use crate::log_stream::Event;
use crate::process_manager::{force_kill_process_group, kill_process_group, run_app_thread, AppStatus, RunningHandle};

pub use crate::app_draft::AppDraft;

mod control_handler;

const MAX_LOG_LINES: usize = 5000;
const SAMPLE_INTERVAL: Duration = Duration::from_millis(1000);
const CPU_CHANGE_THRESHOLD: f32 = 0.5;
const MEMORY_CHANGE_THRESHOLD_MB: f64 = 1.0;
/// Frequence a laquelle on regarde si `apps.json` a ete modifie par un autre process (CLI).
const RELOAD_CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Delai laisse a un process apres SIGTERM avant SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// Le handover attend l'arret des process (SIGKILL compris) pour liberer leurs ports.
const HANDOVER_TIMEOUT: Duration = Duration::from_secs(20);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// Ignore le bruit de mesure sous le seuil pour eviter de bump la revision pour rien.
fn resource_changed(old_cpu: f32, new_cpu: f32, old_mem: f64, new_mem: f64) -> bool {
    (old_cpu - new_cpu).abs() > CPU_CHANGE_THRESHOLD || (old_mem - new_mem).abs() > MEMORY_CHANGE_THRESHOLD_MB
}

struct AppRuntime {
    status: AppStatus,
    logs: VecDeque<String>,
    log_base_seq: u64,
    healthy: Option<bool>,
    cpu_percent: f32,
    memory_mb: f64,
}

impl AppRuntime {
    fn new() -> Self {
        Self {
            status: AppStatus::Stopped,
            logs: VecDeque::new(),
            log_base_seq: 0,
            healthy: None,
            cpu_percent: 0.0,
            memory_mb: 0.0,
        }
    }

    fn push_log(&mut self, line: String) {
        self.logs.push_back(line);
        while self.logs.len() > MAX_LOG_LINES {
            self.logs.pop_front();
            self.log_base_seq += 1;
        }
    }

    /// Avance log_base_seq au-dela des lignes retirees, pour forcer un client en retard
    /// vers un remplacement complet plutot qu'un silencieux "rien de nouveau".
    fn clear_logs(&mut self) {
        self.log_base_seq += self.logs.len() as u64;
        self.logs.clear();
    }

    /// (lignes, sequence de la premiere, remplacement complet ?) pour un client a jour jusqu'a `since_seq`.
    fn lines_since(&self, since_seq: u64) -> (Vec<String>, u64, bool) {
        if since_seq >= self.log_base_seq {
            let skip = (since_seq - self.log_base_seq) as usize;
            (self.logs.iter().skip(skip).cloned().collect(), since_seq, false)
        } else {
            (self.logs.iter().cloned().collect(), self.log_base_seq, true)
        }
    }

    fn next_seq(&self) -> u64 {
        self.log_base_seq + self.logs.len() as u64
    }
}

/// Vue serialisable d'une app, consommee par chaque frontend (GTK direct, ou FFI JSON).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppView {
    pub id: Uuid,
    pub name: String,
    pub working_dir: String,
    pub kind: AppKind,
    pub command: String,
    pub url: Option<String>,
    pub env_vars: Vec<(String, String)>,
    pub auto_restart: bool,
    pub start_order: i32,
    pub status_label: &'static str,
    pub error: Option<String>,
    pub active: bool,
    pub logs: Vec<String>,
    pub logs_base_seq: u64,
    pub logs_replace: bool,
    /// `None` tant que l'app ne tourne pas ou n'a pas d'URL configuree ; sinon refletes
    /// le dernier ping reussi/echoue vers `url`.
    pub healthy: Option<bool>,
    pub cpu_percent: f32,
    pub memory_mb: f64,
}

fn status_label(status: &AppStatus) -> &'static str {
    match status {
        AppStatus::Stopped => "stopped",
        AppStatus::Building => "building",
        AppStatus::Running => "running",
        AppStatus::Failed(_) => "failed",
    }
}

/// Facade sur la config, les process en cours et leurs logs — pas de dependance UI.
pub struct Engine {
    config: AppConfigList,
    runtimes: HashMap<Uuid, AppRuntime>,
    handles: HashMap<Uuid, RunningHandle>,
    event_tx: mpsc::Sender<Event>,
    event_rx: mpsc::Receiver<Event>,
    sys: sysinfo::System,
    /// Reused across sample_resource_usage calls instead of allocating fresh each time.
    children_scratch: HashMap<sysinfo::Pid, Vec<sysinfo::Pid>>,
    stack_scratch: Vec<sysinfo::Pid>,
    visited_scratch: std::collections::HashSet<sysinfo::Pid>,
    last_sample: Option<Instant>,
    revision: u64,
    /// `None` en tests : evite d'ecraser le fichier de config reel de l'utilisateur.
    config_file: Option<ConfigFile>,
    /// Derniere version du fichier connue, pour detecter les ecritures du CLI.
    config_signature: Option<FileSignature>,
    last_reload_check: Option<Instant>,
    /// Apps a relancer des que leur process actuel s'est termine.
    pending_restart: HashSet<Uuid>,
    /// Apps mises en file par `start_all` dont le palier n'est pas encore atteint.
    queued_starts: HashSet<Uuid>,
    /// Echeance apres laquelle un arret demande passe a SIGKILL.
    stop_deadlines: HashMap<Uuid, Instant>,
    control: Option<ControlServer>,
    control_in_flight: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
    handover: Option<PendingHandover>,
    shutdown_requested: bool,
    shutting_down: bool,
}

struct PendingHandover {
    reply: mpsc::Sender<crate::control::ControlResponse>,
    ids: Vec<Uuid>,
    deadline: Instant,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Moteur d'une app native : config par defaut (ou `SWITCHBOARD_CONFIG_DIR`), et canal de
    /// controle pour le CLI. Si le daemon du CLI tournait, il rend la main et ses apps sont relancees ici.
    pub fn new() -> Self {
        match ConfigFile::default_location() {
            Ok(file) => Self::supervising(file),
            Err(_) => Self::build(None),
        }
    }

    /// Comme `new`, sur un fichier donne : reprend la main sur le daemon s'il tourne et
    /// ouvre le canal de controle du CLI.
    pub fn supervising(file: ConfigFile) -> Self {
        let mut engine = Self::build(Some(file));
        engine.become_supervisor();
        engine
    }

    /// Moteur sans canal de controle, sur un fichier donne.
    pub fn with_config_file(file: ConfigFile) -> Self {
        Self::build(Some(file))
    }

    /// Moteur du daemon headless. Refuse de demarrer si un superviseur (app native ou
    /// autre daemon) repond deja pour ce dossier de config.
    pub fn new_daemon(config_dir: &Path) -> Result<Self, DaemonStartError> {
        if let Some(client) = SupervisorClient::discover(config_dir) {
            return Err(DaemonStartError::AlreadySupervised { role: client.endpoint().role, pid: client.endpoint().pid });
        }
        let mut engine = Self::build(Some(ConfigFile::in_dir(config_dir)));
        let server = ControlServer::start(config_dir, SupervisorRole::Daemon, engine.event_tx.clone())
            .map_err(DaemonStartError::Io)?;
        engine.control_in_flight = Some(server.in_flight());
        engine.control = Some(server);
        Ok(engine)
    }

    #[cfg(test)]
    fn new_ephemeral() -> Self {
        Self::build(None)
    }

    fn build(config_file: Option<ConfigFile>) -> Self {
        let config = match config_file.as_ref().map(ConfigFile::load) {
            Some(Ok(list)) => list,
            Some(Err(e)) => {
                eprintln!("switchboard: {e} — starting with an empty list, the file is left untouched");
                AppConfigList::default()
            }
            None => AppConfigList::default(),
        };
        let config_signature = config_file.as_ref().and_then(ConfigFile::signature);
        let mut runtimes = HashMap::new();
        for app in &config.apps {
            runtimes.insert(app.id, AppRuntime::new());
        }
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            config,
            runtimes,
            handles: HashMap::new(),
            event_tx,
            event_rx,
            sys: sysinfo::System::new(),
            children_scratch: HashMap::new(),
            stack_scratch: Vec::new(),
            visited_scratch: std::collections::HashSet::new(),
            last_sample: None,
            revision: 0,
            config_file,
            config_signature,
            last_reload_check: Some(Instant::now()),
            pending_restart: HashSet::new(),
            queued_starts: HashSet::new(),
            stop_deadlines: HashMap::new(),
            control: None,
            control_in_flight: None,
            handover: None,
            shutdown_requested: false,
            shutting_down: false,
        }
    }

    fn become_supervisor(&mut self) {
        let Some(dir) = self.config_file.as_ref().map(|f| f.dir().to_path_buf()) else { return };
        let mut resume = Vec::new();
        if let Some(client) = SupervisorClient::discover(&dir) {
            match client.endpoint().role {
                SupervisorRole::Daemon => match client.request_as::<Vec<Uuid>>(&ControlRequest::Handover, HANDOVER_TIMEOUT) {
                    Ok(ids) => resume = ids,
                    Err(e) => {
                        eprintln!("switchboard: the CLI daemon did not hand over ({e}); CLI control stays with it");
                        return;
                    }
                },
                SupervisorRole::App => {
                    eprintln!("switchboard: another Switchboard window already answers the CLI");
                    return;
                }
            }
        }
        match ControlServer::start(&dir, SupervisorRole::App, self.event_tx.clone()) {
            Ok(server) => {
                self.control_in_flight = Some(server.in_flight());
                self.control = Some(server);
            }
            Err(e) => eprintln!("switchboard: CLI control channel unavailable: {e}"),
        }
        for id in resume {
            self.start_app(id);
        }
    }

    pub fn config_path(&self) -> Option<&Path> {
        self.config_file.as_ref().map(ConfigFile::path)
    }

    /// Lit le fichier si son empreinte a change (au plus une fois par seconde, sauf `force`)
    /// et reconcilie : apps supprimees arretees, nouvelles apps pretes a demarrer.
    fn reload_config_if_changed(&mut self, force: bool) {
        let Some(file) = &self.config_file else { return };
        let now = Instant::now();
        if !force && self.last_reload_check.is_some_and(|t| now.duration_since(t) < RELOAD_CHECK_INTERVAL) {
            return;
        }
        self.last_reload_check = Some(now);
        let signature = file.signature();
        // Fichier supprime : on garde la config en memoire plutot que de tout arreter.
        if signature.is_none() || signature == self.config_signature {
            return;
        }
        self.config_signature = signature;
        if let Ok(list) = file.load()
            && list != self.config
        {
            self.apply_config(list);
        }
    }

    fn apply_config(&mut self, list: AppConfigList) {
        let removed: Vec<Uuid> =
            self.config.apps.iter().map(|a| a.id).filter(|id| list.find(*id).is_none()).collect();
        for id in removed {
            self.forget_app(id);
        }
        for app in &list.apps {
            self.runtimes.entry(app.id).or_insert_with(AppRuntime::new);
        }
        self.config = list;
        self.revision += 1;
    }

    fn forget_app(&mut self, id: Uuid) {
        self.pending_restart.remove(&id);
        self.queued_starts.remove(&id);
        self.stop_app(id);
        self.runtimes.remove(&id);
    }

    /// Recharge, modifie et sauvegarde sous verrou : une ecriture du CLI entre la lecture
    /// et la sauvegarde ne peut pas etre perdue.
    fn mutate_config<T>(&mut self, mutate: impl FnOnce(&mut AppConfigList) -> T) -> T {
        let Some(file) = self.config_file.clone() else { return mutate(&mut self.config) };
        let lock = file.lock();
        if let Err(e) = &lock {
            eprintln!("switchboard: {e}");
        }
        self.reload_config_if_changed(true);
        let result = mutate(&mut self.config);
        self.persist(&file);
        drop(lock);
        self.revision += 1;
        result
    }

    fn persist(&mut self, file: &ConfigFile) {
        if let Err(ConfigError::Invalid { .. }) = file.load() {
            match file.backup_invalid() {
                Ok(backup) => eprintln!("switchboard: invalid config saved as {}", backup.display()),
                Err(e) => {
                    eprintln!("switchboard: not overwriting invalid config (backup failed: {e})");
                    return;
                }
            }
        }
        match file.save(&self.config) {
            Ok(()) => self.config_signature = file.signature(),
            Err(e) => eprintln!("switchboard: could not save config: {e}"),
        }
    }

    pub fn drain_events(&mut self) {
        let mut processed = false;
        while let Ok(event) = self.event_rx.try_recv() {
            processed = true;
            match event {
                Event::Log(id, line) => {
                    if let Some(rt) = self.runtimes.get_mut(&id) {
                        rt.push_log(line);
                    }
                }
                Event::StatusChanged(id, status) => {
                    let terminal = !matches!(status, AppStatus::Running | AppStatus::Building);
                    if let Some(rt) = self.runtimes.get_mut(&id) {
                        rt.status = status;
                        if terminal {
                            rt.healthy = None;
                            rt.cpu_percent = 0.0;
                            rt.memory_mb = 0.0;
                        }
                    }
                    if terminal {
                        self.handles.remove(&id);
                        self.stop_deadlines.remove(&id);
                        if self.pending_restart.remove(&id) && !self.shutting_down {
                            self.start_app_now(id);
                        }
                    }
                }
                Event::HealthChanged(id, healthy) => {
                    if let Some(rt) = self.runtimes.get_mut(&id) {
                        rt.healthy = Some(healthy);
                    }
                }
                Event::StartRequested(id) => {
                    let queued = self.queued_starts.remove(&id);
                    if queued && !self.handles.contains_key(&id) && !self.shutting_down {
                        self.start_app_now(id);
                    }
                }
                Event::Control(request, reply) => self.handle_control(request, reply),
            }
        }
        if processed {
            self.revision += 1;
        }
    }

    fn poll(&mut self) {
        self.reload_config_if_changed(false);
        self.drain_events();
        self.enforce_stop_deadlines();
        self.finish_handover_if_ready();
        self.sample_resource_usage();
    }

    /// Pour une boucle sans UI (daemon) : traite evenements, rechargements et requetes du CLI.
    pub fn tick(&mut self) {
        self.poll();
    }

    pub fn has_active_apps(&self) -> bool {
        !self.handles.is_empty()
    }

    /// Le daemon doit se terminer (requete `shutdown` ou `handover` du CLI / d'une app native).
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    fn enforce_stop_deadlines(&mut self) {
        if self.stop_deadlines.is_empty() {
            return;
        }
        let now = Instant::now();
        let expired: Vec<Uuid> = self.stop_deadlines.iter().filter(|(_, d)| now >= **d).map(|(id, _)| *id).collect();
        for id in expired {
            self.stop_deadlines.remove(&id);
            if let Some(pgid) = self.handles.get(&id).and_then(|h| *h.pgid.lock().unwrap()) {
                force_kill_process_group(pgid);
            }
        }
    }

    /// Echantillonne CPU/memoire des process actifs, au plus une fois par seconde.
    fn sample_resource_usage(&mut self) {
        let now = Instant::now();
        if self.last_sample.is_some_and(|t| now.duration_since(t) < SAMPLE_INTERVAL) {
            return;
        }
        self.last_sample = Some(now);

        let roots: Vec<sysinfo::Pid> = self
            .handles
            .values()
            .filter_map(|h| *h.pgid.lock().unwrap())
            .map(|pid| sysinfo::Pid::from_u32(pid as u32))
            .collect();
        if roots.is_empty() {
            return;
        }
        // Whole table needed for the parent/child tree — real memory lives in subprocesses.
        self.sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            true,
            sysinfo::ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );

        let mut children = std::mem::take(&mut self.children_scratch);
        children.clear();
        for (pid, process) in self.sys.processes() {
            if let Some(parent) = process.parent() {
                children.entry(parent).or_default().push(*pid);
            }
        }

        let mut changed = false;
        for (id, handle) in &self.handles {
            let Some(root) = *handle.pgid.lock().unwrap() else { continue };
            let root = sysinfo::Pid::from_u32(root as u32);
            if self.sys.process(root).is_none() {
                continue;
            }
            let mut cpu = 0.0f32;
            let mut mem_bytes = 0u64;
            self.stack_scratch.clear();
            self.stack_scratch.push(root);
            self.visited_scratch.clear();
            while let Some(pid) = self.stack_scratch.pop() {
                if !self.visited_scratch.insert(pid) {
                    continue;
                }
                if let Some(process) = self.sys.process(pid) {
                    cpu += process.cpu_usage();
                    mem_bytes += process.memory();
                }
                if let Some(kids) = children.get(&pid) {
                    self.stack_scratch.extend(kids.iter().copied());
                }
            }
            if let Some(rt) = self.runtimes.get_mut(id) {
                let mem = mem_bytes as f64 / 1_048_576.0;
                if resource_changed(rt.cpu_percent, cpu, rt.memory_mb, mem) {
                    changed = true;
                }
                rt.cpu_percent = cpu;
                rt.memory_mb = mem;
            }
        }
        self.children_scratch = children;
        if changed {
            self.revision += 1;
        }
    }

    pub fn list_apps(&mut self, logs_for: Option<(Uuid, u64)>) -> Vec<AppView> {
        self.poll();
        self.views(logs_for)
    }

    fn views(&self, logs_for: Option<(Uuid, u64)>) -> Vec<AppView> {
        self.config
            .apps
            .iter()
            .map(|app| {
                let runtime = self.runtimes.get(&app.id);
                let status = runtime.map(|r| r.status.clone()).unwrap_or(AppStatus::Stopped);
                let active = matches!(status, AppStatus::Running | AppStatus::Building);
                let error = match &status {
                    AppStatus::Failed(msg) => Some(msg.clone()),
                    _ => None,
                };
                let (logs, logs_base_seq, logs_replace) = match (logs_for, runtime) {
                    (Some((id, since_seq)), Some(rt)) if id == app.id => rt.lines_since(since_seq),
                    _ => (Vec::new(), 0, false),
                };
                AppView {
                    id: app.id,
                    name: app.name.clone(),
                    working_dir: app.working_dir.display().to_string(),
                    kind: app.kind,
                    command: app.command.clone(),
                    url: app.url.clone(),
                    env_vars: app.env_vars.clone(),
                    auto_restart: app.auto_restart,
                    start_order: app.start_order,
                    status_label: status_label(&status),
                    error,
                    active,
                    logs,
                    logs_base_seq,
                    logs_replace,
                    healthy: runtime.and_then(|r| r.healthy),
                    cpu_percent: runtime.map(|r| r.cpu_percent).unwrap_or(0.0),
                    memory_mb: runtime.map(|r| r.memory_mb).unwrap_or(0.0),
                }
            })
            .collect()
    }

    pub fn add_app(&mut self, draft: AppDraft) -> Uuid {
        let app = draft.into_config(Uuid::new_v4());
        let id = app.id;
        self.runtimes.insert(id, AppRuntime::new());
        self.mutate_config(|config| config.add(app));
        id
    }

    pub fn update_app(&mut self, id: Uuid, draft: AppDraft) {
        self.mutate_config(|config| {
            config.update(id, |app| *app = draft.into_config(id));
        });
    }

    pub fn remove_app(&mut self, id: Uuid) {
        self.mutate_config(|config| config.remove(id));
        self.forget_app(id);
    }

    /// Exporte un sous-ensemble d'apps en JSON, pret a etre ecrit dans un fichier.
    pub fn export_config(&self, ids: &[Uuid], include_env_vars: bool) -> String {
        self.config.export_subset(ids, include_env_vars).to_json().unwrap_or_default()
    }

    /// Apercu de fusion sans rien modifier. `None` si `json` n'est pas un `AppConfigList` valide.
    pub fn preview_import(&self, json: &str) -> Option<ImportSummary> {
        let incoming = AppConfigList::from_json(json).ok()?;
        Some(self.config.clone().merge_import(incoming))
    }

    /// Applique la fusion et persiste. Les apps remplacees gardent leur runtime existant.
    pub fn apply_import(&mut self, json: &str) -> Option<ImportSummary> {
        let incoming = AppConfigList::from_json(json).ok()?;
        let summary = self.mutate_config(|config| config.merge_import(incoming));
        for app in &self.config.apps {
            self.runtimes.entry(app.id).or_insert_with(AppRuntime::new);
        }
        Some(summary)
    }

    pub fn clear_logs(&mut self, id: Uuid) {
        if let Some(rt) = self.runtimes.get_mut(&id) {
            rt.clear_logs();
            self.revision += 1;
        }
    }

    /// Ecrit les logs courants d'une app dans un fichier (une ligne par entree).
    pub fn export_logs(&self, id: Uuid, path: &std::path::Path) -> std::io::Result<()> {
        let Some(rt) = self.runtimes.get(&id) else {
            return Ok(());
        };
        let content = rt.logs.iter().cloned().collect::<Vec<_>>().join("\n");
        std::fs::write(path, content)
    }

    /// Sans effet si l'app tourne deja (evite deux instances qui se disputent un port).
    pub fn start_app(&mut self, id: Uuid) -> ActionOutcome {
        if self.config.find(id).is_none() {
            return ActionOutcome::NotFound;
        }
        if self.handles.contains_key(&id) {
            return ActionOutcome::AlreadyRunning;
        }
        self.start_app_now(id)
    }

    fn start_app_now(&mut self, id: Uuid) -> ActionOutcome {
        let Some(config) = self.config.find(id).cloned() else {
            return ActionOutcome::NotFound;
        };
        self.revision += 1;
        if !config.working_dir.is_dir() {
            if let Some(rt) = self.runtimes.get_mut(&id) {
                rt.status = AppStatus::Failed(format!("dossier introuvable: {}", config.working_dir.display()));
            }
            return ActionOutcome::Failed;
        }
        // Statut immediat (avant le premier evenement du thread) : un `wait` ou un rafraichissement
        // juste apres le demarrage ne doit pas voir l'echec du lancement precedent.
        if let Some(rt) = self.runtimes.get_mut(&id) {
            rt.status = if config.kind == AppKind::Cargo { AppStatus::Building } else { AppStatus::Running };
            rt.healthy = None;
        }
        let handle = RunningHandle::new();
        let pgid_slot = handle.pgid.clone();
        let stop_requested = handle.stop_requested.clone();
        if let Some(url) = config.url.clone() {
            spawn_health_watcher(id, url, stop_requested.clone(), self.event_tx.clone());
        }
        self.handles.insert(id, handle);
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            run_app_thread(config, pgid_slot, stop_requested, tx);
        });
        ActionOutcome::Started
    }

    /// SIGTERM au groupe de process, puis SIGKILL s'il tourne encore apres `STOP_GRACE`.
    pub fn stop_app(&mut self, id: Uuid) -> ActionOutcome {
        self.pending_restart.remove(&id);
        self.queued_starts.remove(&id);
        let Some(handle) = self.handles.get(&id) else {
            return if self.config.find(id).is_some() { ActionOutcome::NotRunning } else { ActionOutcome::NotFound };
        };
        handle.stop_requested.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(pgid) = *handle.pgid.lock().unwrap() {
            kill_process_group(pgid);
        }
        self.stop_deadlines.entry(id).or_insert_with(|| Instant::now() + STOP_GRACE);
        ActionOutcome::Stopping
    }

    /// Arrete puis relance avec la config courante (une modification n'est prise en compte
    /// qu'au prochain demarrage).
    pub fn restart_app(&mut self, id: Uuid) -> ActionOutcome {
        if self.config.find(id).is_none() {
            return ActionOutcome::NotFound;
        }
        if !self.handles.contains_key(&id) {
            return self.start_app(id);
        }
        self.stop_app(id);
        self.pending_restart.insert(id);
        ActionOutcome::Restarting
    }

    /// Demarre par paliers de `start_order` croissant, chacun attendant le precedent.
    /// Non-bloquant : tourne sur son propre thread, qui pousse des `Event::StartRequested`
    /// que seul `drain_events` consomme. Retourne les apps mises en file.
    pub fn start_all(&mut self) -> Vec<Uuid> {
        let mut tiers: Vec<(i32, Uuid)> = self
            .config
            .apps
            .iter()
            .filter(|a| !self.handles.contains_key(&a.id))
            .map(|a| (a.start_order, a.id))
            .collect();
        tiers.sort_by_key(|(order, _)| *order);
        let queued: Vec<Uuid> = tiers.iter().map(|(_, id)| *id).collect();
        self.queued_starts.extend(queued.iter().copied());

        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let mut current_order: Option<i32> = None;
            for (order, id) in tiers {
                if current_order.is_some_and(|prev| prev != order) {
                    std::thread::sleep(Duration::from_millis(400));
                }
                current_order = Some(order);
                if tx.send(Event::StartRequested(id)).is_err() {
                    return;
                }
            }
        });
        queued
    }

    pub fn stop_all_running(&mut self) -> Vec<Uuid> {
        let ids: Vec<Uuid> = self.handles.keys().copied().collect();
        for id in &ids {
            self.stop_app(*id);
        }
        ids
    }

    /// Arrete tout et attend la fin des process (SIGKILL au-dela de `grace`), pour ne pas
    /// laisser d'orphelins qui gardent leurs ports.
    pub fn shutdown(&mut self, grace: Duration) {
        self.shutting_down = true;
        self.control = None;
        self.stop_all_running();
        let deadline = Instant::now() + grace;
        while !self.handles.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            self.drain_events();
        }
        for handle in self.handles.values() {
            if let Some(pgid) = *handle.pgid.lock().unwrap() {
                force_kill_process_group(pgid);
            }
        }
        // Repond "shutting_down" aux requetes en attente, puis laisse les reponses deja
        // envoyees (shutdown, handover) s'ecrire avant que le process ne se termine.
        self.drain_events();
        if let Some(in_flight) = &self.control_in_flight {
            let deadline = Instant::now() + Duration::from_secs(2);
            while in_flight.load(std::sync::atomic::Ordering::SeqCst) > 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    /// Cheap poll target — callers re-fetch `list_apps` only when this changes.
    pub fn revision(&mut self) -> u64 {
        self.poll();
        self.revision
    }
}

#[derive(Debug)]
pub enum DaemonStartError {
    AlreadySupervised { role: SupervisorRole, pid: u32 },
    Io(std::io::Error),
}

impl std::fmt::Display for DaemonStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaemonStartError::AlreadySupervised { role, pid } => {
                write!(f, "apps are already supervised by the {} (pid {pid})", role.as_str())
            }
            DaemonStartError::Io(e) => write!(f, "cannot open the control channel: {e}"),
        }
    }
}

impl std::error::Error for DaemonStartError {}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown(SHUTDOWN_GRACE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::AppConfig;
    use std::path::{Path, PathBuf};

    fn temp_engine() -> Engine {
        Engine::new_ephemeral()
    }

    fn base_draft() -> AppDraft {
        AppDraft {
            name: "X".to_string(),
            working_dir: PathBuf::from("/tmp"),
            kind: AppKind::Raw,
            command: "".to_string(),
            url: None,
            env_vars: vec![],
            auto_restart: false,
            start_order: 0,
        }
    }

    #[test]
    fn add_app_persists_new_fields() {
        let mut engine = temp_engine();
        let id = engine.add_app(AppDraft {
            name: "Custom".to_string(),
            url: Some("http://localhost:9999".to_string()),
            env_vars: vec![("FOO".to_string(), "bar".to_string())],
            auto_restart: true,
            start_order: 2,
            ..base_draft()
        });
        let apps = engine.list_apps(None);
        let app = apps.iter().find(|a| a.id == id).expect("app present");
        assert_eq!(app.url, Some("http://localhost:9999".to_string()));
        assert_eq!(app.env_vars, vec![("FOO".to_string(), "bar".to_string())]);
        assert!(app.auto_restart);
        assert_eq!(app.start_order, 2);
        assert_eq!(app.healthy, None);
    }

    #[test]
    fn update_app_overwrites_fields() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());
        engine.update_app(id, AppDraft {
            name: "Y".to_string(),
            kind: AppKind::Npm,
            command: "start".to_string(),
            url: Some("http://localhost:1234".to_string()),
            env_vars: vec![("A".to_string(), "B".to_string())],
            auto_restart: true,
            start_order: 3,
            ..base_draft()
        });
        let apps = engine.list_apps(None);
        let app = apps.iter().find(|a| a.id == id).expect("app present");
        assert_eq!(app.name, "Y");
        assert_eq!(app.url, Some("http://localhost:1234".to_string()));
        assert!(app.auto_restart);
        assert_eq!(app.start_order, 3);
    }

    #[test]
    fn export_config_returns_json_containing_only_selected_ids() {
        let mut engine = temp_engine();
        let kept_id = engine.add_app(AppDraft { name: "Kept".to_string(), ..base_draft() });
        engine.add_app(AppDraft { name: "Dropped".to_string(), ..base_draft() });

        let json = engine.export_config(&[kept_id], false);
        let exported = AppConfigList::from_json(&json).expect("valid json");

        assert_eq!(exported.apps.len(), 1);
        assert_eq!(exported.apps[0].name, "Kept");
    }

    #[test]
    fn export_config_strips_env_vars_when_excluded() {
        let mut engine = temp_engine();
        let id = engine.add_app(AppDraft {
            name: "X".to_string(),
            env_vars: vec![("K".to_string(), "V".to_string())],
            ..base_draft()
        });

        let json = engine.export_config(&[id], false);
        let exported = AppConfigList::from_json(&json).expect("valid json");

        assert!(exported.apps[0].env_vars.is_empty());
    }

    #[test]
    fn preview_import_does_not_mutate_engine_config() {
        let mut engine = temp_engine();
        engine.add_app(AppDraft { name: "Existing".to_string(), ..base_draft() });

        let mut incoming = AppConfigList::default();
        incoming.add(AppConfig { name: "New One".to_string(), ..Default::default() });
        let json = incoming.to_json().expect("serialize");

        let summary = engine.preview_import(&json).expect("valid json");

        assert_eq!(summary.to_add, vec!["New One".to_string()]);
        assert_eq!(engine.config.apps.len(), 1);
        assert_eq!(engine.config.apps[0].name, "Existing");
    }

    #[test]
    fn apply_import_adds_new_apps_and_seeds_their_runtime() {
        let mut engine = temp_engine();
        let mut incoming = AppConfigList::default();
        incoming.add(AppConfig { name: "New One".to_string(), ..Default::default() });
        let json = incoming.to_json().expect("serialize");

        let summary = engine.apply_import(&json).expect("valid json");

        assert_eq!(summary.to_add, vec!["New One".to_string()]);
        assert_eq!(engine.config.apps.len(), 1);
        let new_id = engine.config.apps[0].id;
        assert!(engine.runtimes.contains_key(&new_id));
    }

    #[test]
    fn apply_import_replaces_existing_app_preserving_id_and_runtime() {
        let mut engine = temp_engine();
        let existing_id = engine.add_app(AppDraft { name: "Alume API".to_string(), ..base_draft() });

        let mut incoming = AppConfigList::default();
        incoming.add(AppConfig { name: "Alume API".to_string(), command: "new-command".to_string(), ..Default::default() });
        let json = incoming.to_json().expect("serialize");

        let summary = engine.apply_import(&json).expect("valid json");

        assert_eq!(summary.to_replace, vec!["Alume API".to_string()]);
        assert_eq!(engine.config.apps.len(), 1);
        assert_eq!(engine.config.apps[0].id, existing_id);
        assert_eq!(engine.config.apps[0].command, "new-command");
        assert!(engine.runtimes.contains_key(&existing_id));
    }

    #[test]
    fn apply_import_returns_none_for_invalid_json_and_does_not_mutate() {
        let mut engine = temp_engine();
        engine.add_app(AppDraft { name: "Existing".to_string(), ..base_draft() });

        assert!(engine.apply_import("not valid json").is_none());
        assert_eq!(engine.config.apps.len(), 1);
        assert_eq!(engine.config.apps[0].name, "Existing");
    }

    #[test]
    fn clear_logs_empties_log_buffer() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());
        engine.clear_logs(id);
        let apps = engine.list_apps(None);
        assert!(apps.iter().find(|a| a.id == id).unwrap().logs.is_empty());
    }

    #[test]
    fn start_all_returns_without_blocking_the_calling_thread() {
        let mut engine = temp_engine();
        // Two tiers means a 400ms inter-tier delay — this must not block start_all itself.
        engine.add_app(AppDraft { start_order: 0, working_dir: Path::new("/nonexistent-a").to_path_buf(), ..base_draft() });
        engine.add_app(AppDraft { start_order: 1, working_dir: Path::new("/nonexistent-b").to_path_buf(), ..base_draft() });

        let started = Instant::now();
        engine.start_all();
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "start_all must not block the calling thread on the inter-tier delay"
        );
    }

    #[test]
    fn start_all_skips_already_running_apps() {
        let mut engine = temp_engine();
        // The nonexistent dir makes start_app fail immediately — just checking start_all doesn't panic.
        engine.add_app(AppDraft {
            working_dir: Path::new("/nonexistent").to_path_buf(),
            ..base_draft()
        });
        engine.start_all();
        let apps = engine.list_apps(None);
        assert_eq!(apps.len(), 1);
    }

    #[test]
    fn export_logs_writes_lines_to_file() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());
        // Simule des logs via le canal d'evenements directement (pas de vrai process).
        engine.drain_events();
        let tmp = std::env::temp_dir().join(format!("switchboard-test-{id}.log"));
        engine.export_logs(id, &tmp).expect("export should succeed");
        assert!(tmp.exists());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn revision_bumps_only_when_an_event_is_actually_drained() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());
        let before = engine.revision();
        assert_eq!(before, engine.revision(), "no events pending, revision must stay flat");

        engine.event_tx.send(Event::Log(id, "hello".to_string())).unwrap();
        let after = engine.revision();
        assert!(after > before, "a drained log event must bump the revision");
    }

    #[test]
    fn list_apps_only_includes_logs_for_the_requested_id() {
        let mut engine = temp_engine();
        let id_a = engine.add_app(base_draft());
        let id_b = engine.add_app(base_draft());
        engine.event_tx.send(Event::Log(id_a, "from a".to_string())).unwrap();
        engine.event_tx.send(Event::Log(id_b, "from b".to_string())).unwrap();

        let apps = engine.list_apps(Some((id_a, 0)));
        let a = apps.iter().find(|a| a.id == id_a).unwrap();
        let b = apps.iter().find(|a| a.id == id_b).unwrap();
        assert_eq!(a.logs, vec!["from a".to_string()]);
        assert!(b.logs.is_empty(), "non-selected app must not ship its logs");
    }

    #[test]
    fn list_apps_delta_returns_only_new_lines_when_client_is_caught_up() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());
        engine.event_tx.send(Event::Log(id, "line 1".to_string())).unwrap();
        engine.event_tx.send(Event::Log(id, "line 2".to_string())).unwrap();
        let first = engine.list_apps(Some((id, 0)));
        let entry = first.iter().find(|a| a.id == id).unwrap();
        assert_eq!(entry.logs, vec!["line 1".to_string(), "line 2".to_string()]);
        assert!(!entry.logs_replace);
        assert_eq!(entry.logs_base_seq, 0);

        engine.event_tx.send(Event::Log(id, "line 3".to_string())).unwrap();
        let since = entry.logs_base_seq + entry.logs.len() as u64; // client's new known sequence
        let second = engine.list_apps(Some((id, since)));
        let entry2 = second.iter().find(|a| a.id == id).unwrap();
        assert_eq!(entry2.logs, vec!["line 3".to_string()]);
        assert!(!entry2.logs_replace);
        assert_eq!(entry2.logs_base_seq, since);
    }

    #[test]
    fn list_apps_treats_a_since_seq_ahead_of_reality_as_caught_up_not_a_replace() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());
        // since_seq (100) ahead of log_base_seq (0) must still take the delta path, not replace.
        let apps = engine.list_apps(Some((id, 100)));
        let entry = apps.iter().find(|a| a.id == id).unwrap();
        assert!(entry.logs.is_empty());
        assert!(!entry.logs_replace);
    }

    #[test]
    fn list_apps_forces_full_replace_when_client_is_behind_evicted_lines() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());
        // Push one more line than MAX_LOG_LINES so the oldest line is evicted and
        // log_base_seq advances past 0.
        for i in 0..(MAX_LOG_LINES + 1) {
            engine.event_tx.send(Event::Log(id, format!("line {i}"))).unwrap();
        }

        // A client that last synced at sequence 0 has missed the evicted line —
        // it must get a full replace, not a delta computed from a negative/invalid offset.
        let apps = engine.list_apps(Some((id, 0)));
        let entry = apps.iter().find(|a| a.id == id).unwrap();
        assert!(entry.logs_replace);
        assert_eq!(entry.logs.len(), MAX_LOG_LINES);
        assert_eq!(entry.logs.first(), Some(&"line 1".to_string())); // "line 0" was evicted
        assert_eq!(entry.logs.last(), Some(&format!("line {MAX_LOG_LINES}")));
        assert_eq!(entry.logs_base_seq, 1); // log_base_seq advanced by exactly one eviction
    }

    #[test]
    fn clear_logs_advances_base_seq_so_a_stale_client_gets_a_replace() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());
        engine.event_tx.send(Event::Log(id, "a".to_string())).unwrap();
        engine.event_tx.send(Event::Log(id, "b".to_string())).unwrap();
        let first = engine.list_apps(Some((id, 0)));
        let entry = first.iter().find(|a| a.id == id).unwrap();
        let caught_up_since = entry.logs_base_seq + entry.logs.len() as u64; // == 2

        engine.clear_logs(id);

        // A client that was caught up before the clear (since=2) must see an
        // empty delta, not a replace — the buffer really is empty now.
        let after_clear = engine.list_apps(Some((id, caught_up_since)));
        let entry2 = after_clear.iter().find(|a| a.id == id).unwrap();
        assert!(entry2.logs.is_empty());
        assert!(!entry2.logs_replace);

        // A client that was stale before the clear (since=0, missed both lines)
        // must be told to replace, not silently see nothing.
        engine.event_tx.send(Event::Log(id, "c".to_string())).unwrap();
        let stale_client = engine.list_apps(Some((id, 0)));
        let entry3 = stale_client.iter().find(|a| a.id == id).unwrap();
        assert!(entry3.logs_replace);
        assert_eq!(entry3.logs, vec!["c".to_string()]);
    }

    #[test]
    fn list_apps_with_no_selection_ships_no_logs_at_all() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());
        engine.event_tx.send(Event::Log(id, "hello".to_string())).unwrap();

        let apps = engine.list_apps(None);
        assert!(apps.iter().find(|a| a.id == id).unwrap().logs.is_empty());
    }

    #[test]
    fn handles_entry_is_removed_once_a_process_stops_so_start_all_can_restart_it() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());

        // start_app's real path inserts into `handles` before the process thread runs;
        // simulate that directly rather than spawning a real process.
        engine.handles.insert(id, RunningHandle::new());
        assert!(engine.handles.contains_key(&id), "sanity: handles has the entry before the stop event");

        engine.event_tx.send(Event::StatusChanged(id, AppStatus::Stopped)).unwrap();
        engine.drain_events();
        assert!(
            !engine.handles.contains_key(&id),
            "a stopped app must be removed from handles or start_all will refuse to restart it"
        );
    }

    #[test]
    fn start_requested_is_a_no_op_if_the_app_is_already_in_handles() {
        let mut engine = temp_engine();
        let id = engine.add_app(AppDraft {
            working_dir: Path::new("/nonexistent-for-guard-test").to_path_buf(),
            ..base_draft()
        });

        // Simulate the first dispatch's event having already been drained and
        // inserted a handle, as start_app_now's real path does.
        engine.handles.insert(id, RunningHandle::new());

        // A second StartRequested for the same id (e.g. from a rapid double
        // "start all" click) must be a no-op: if the guard were missing,
        // start_app_now would run again and, since the working dir doesn't
        // exist, immediately set status to Failed.
        engine.event_tx.send(Event::StartRequested(id)).unwrap();
        engine.drain_events();

        let apps = engine.list_apps(None);
        let app = apps.iter().find(|a| a.id == id).unwrap();
        assert_ne!(
            app.status_label, "failed",
            "guarded StartRequested must not re-run start_app_now for an id already in handles"
        );
    }

    #[test]
    fn resource_change_detection_respects_thresholds() {
        assert!(!resource_changed(10.0, 10.3, 50.0, 50.5)); // both sub-threshold
        assert!(resource_changed(10.0, 10.6, 50.0, 50.5));  // cpu over threshold
        assert!(resource_changed(10.0, 10.3, 50.0, 51.5));  // memory over threshold
        assert!(!resource_changed(10.0, 10.0, 50.0, 50.0));  // no change at all
    }
    fn file_engine() -> (tempfile::TempDir, ConfigFile, Engine) {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = ConfigFile::in_dir(dir.path());
        let engine = Engine::with_config_file(file.clone());
        (dir, file, engine)
    }

    /// Les mtime de certains systemes de fichiers ont une resolution grossiere : on force
    /// une empreinte differente en changeant aussi la taille.
    fn external_write(file: &ConfigFile, list: &AppConfigList) {
        file.update(|on_disk| {
            *on_disk = list.clone();
            Ok::<_, ConfigError>(())
        })
        .unwrap();
    }

    #[test]
    fn engine_persists_mutations_to_its_config_file() {
        let (_dir, file, mut engine) = file_engine();
        let id = engine.add_app(AppDraft { name: "Api".to_string(), ..base_draft() });
        assert_eq!(file.load().unwrap().find(id).map(|a| a.name.as_str()), Some("Api"));
        engine.remove_app(id);
        assert!(file.load().unwrap().apps.is_empty());
    }

    #[test]
    fn engine_picks_up_apps_added_by_another_process() {
        let (_dir, file, mut engine) = file_engine();
        let before = engine.revision();
        let mut list = AppConfigList::default();
        list.add(AppConfig { name: "From CLI".to_string(), working_dir: PathBuf::from("/tmp"), ..Default::default() });
        external_write(&file, &list);

        engine.reload_config_if_changed(true);
        let apps = engine.list_apps(None);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "From CLI");
        assert!(engine.revision() > before, "a reload must bump the revision so frontends refresh");
    }

    #[test]
    fn engine_reload_drops_apps_removed_by_another_process() {
        let (_dir, file, mut engine) = file_engine();
        let id = engine.add_app(base_draft());
        engine.handles.insert(id, RunningHandle::new());

        external_write(&file, &AppConfigList::default());
        engine.reload_config_if_changed(true);

        assert!(engine.list_apps(None).is_empty());
        assert!(!engine.runtimes.contains_key(&id));
        assert!(engine.handles[&id].stop_requested.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn engine_mutation_does_not_lose_a_concurrent_external_edit() {
        let (_dir, file, mut engine) = file_engine();
        engine.add_app(AppDraft { name: "First".to_string(), ..base_draft() });
        let mut on_disk = file.load().unwrap();
        on_disk.add(AppConfig { name: "Added by CLI".to_string(), ..Default::default() });
        external_write(&file, &on_disk);

        // No poll in between: the mutation itself must reload before saving.
        engine.add_app(AppDraft { name: "Second".to_string(), ..base_draft() });

        let names: Vec<_> = file.load().unwrap().apps.into_iter().map(|a| a.name).collect();
        assert_eq!(names, vec!["First", "Added by CLI", "Second"]);
    }

    #[test]
    fn engine_keeps_a_broken_file_until_it_must_write() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile::in_dir(dir.path());
        std::fs::write(file.path(), "{ typo").unwrap();
        let mut engine = Engine::with_config_file(file.clone());
        assert!(engine.list_apps(None).is_empty());
        assert_eq!(std::fs::read_to_string(file.path()).unwrap(), "{ typo");

        engine.add_app(base_draft());
        assert_eq!(file.load().unwrap().apps.len(), 1);
        let backups = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains(".json.invalid-"))
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn start_app_refuses_a_second_instance() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());
        engine.handles.insert(id, RunningHandle::new());
        assert_eq!(engine.start_app(id), ActionOutcome::AlreadyRunning);
    }

    #[test]
    fn start_app_reports_unknown_ids_and_missing_dirs() {
        let mut engine = temp_engine();
        assert_eq!(engine.start_app(Uuid::new_v4()), ActionOutcome::NotFound);
        let id = engine.add_app(AppDraft { working_dir: PathBuf::from("/nonexistent-dir-42"), ..base_draft() });
        assert_eq!(engine.start_app(id), ActionOutcome::Failed);
        assert_eq!(engine.list_apps(None)[0].status_label, "failed");
    }

    #[test]
    fn stop_app_reports_not_running() {
        let mut engine = temp_engine();
        let id = engine.add_app(base_draft());
        assert_eq!(engine.stop_app(id), ActionOutcome::NotRunning);
        assert_eq!(engine.stop_app(Uuid::new_v4()), ActionOutcome::NotFound);
    }

    #[test]
    fn restart_starts_again_once_the_old_process_has_stopped() {
        let mut engine = temp_engine();
        let id = engine.add_app(AppDraft { working_dir: PathBuf::from("/nonexistent-dir-43"), ..base_draft() });
        engine.handles.insert(id, RunningHandle::new());

        assert_eq!(engine.restart_app(id), ActionOutcome::Restarting);
        assert!(engine.handles.contains_key(&id), "old process still running until its exit event");

        engine.event_tx.send(Event::StatusChanged(id, AppStatus::Stopped)).unwrap();
        engine.drain_events();
        // The relaunch ran (and failed on the missing directory) as soon as the stop landed.
        assert_eq!(engine.list_apps(None)[0].status_label, "failed");
    }

    #[test]
    fn stop_cancels_a_pending_restart() {
        let mut engine = temp_engine();
        let id = engine.add_app(AppDraft { working_dir: PathBuf::from("/nonexistent-dir-44"), ..base_draft() });
        engine.handles.insert(id, RunningHandle::new());
        engine.restart_app(id);
        engine.stop_app(id);
        engine.event_tx.send(Event::StatusChanged(id, AppStatus::Stopped)).unwrap();
        engine.drain_events();
        assert_eq!(engine.list_apps(None)[0].status_label, "stopped");
    }

    #[cfg(unix)]
    #[test]
    fn a_process_ignoring_sigterm_is_killed_after_the_grace_period() {
        let mut engine = temp_engine();
        let id = engine.add_app(AppDraft {
            command: "trap '' TERM; echo ready; while true; do sleep 1; done".to_string(),
            ..base_draft()
        });
        assert_eq!(engine.start_app(id), ActionOutcome::Started);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let view = engine.list_apps(Some((id, 0))).remove(0);
            if view.logs.iter().any(|l| l == "ready") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        engine.stop_app(id);
        // Pretend the grace period is over instead of waiting for it.
        engine.stop_deadlines.insert(id, Instant::now());
        let deadline = Instant::now() + Duration::from_secs(5);
        while engine.has_active_apps() && Instant::now() < deadline {
            engine.tick();
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!engine.has_active_apps(), "SIGKILL escalation must end a process that traps SIGTERM");
    }

    #[test]
    fn control_requests_round_trip_through_a_daemon_engine() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = Engine::new_daemon(dir.path()).expect("daemon engine");
        let id = engine.add_app(base_draft());
        let (done_tx, done_rx) = mpsc::channel();
        let client_dir = dir.path().to_path_buf();
        std::thread::spawn(move || {
            let client = SupervisorClient::discover(&client_dir).expect("supervisor discovered");
            let status: Vec<crate::control::RuntimeState> =
                client.request_as(&ControlRequest::Status, Duration::from_secs(5)).expect("status");
            let logs: crate::control::LogChunk =
                client.request_as(&ControlRequest::Logs { id, since: 0 }, Duration::from_secs(5)).expect("logs");
            let unknown = client.request(&ControlRequest::Logs { id: Uuid::nil(), since: 0 }, Duration::from_secs(5));
            done_tx.send((client.endpoint().role, status, logs, unknown.is_err())).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let result = loop {
            engine.tick();
            if let Ok(result) = done_rx.try_recv() {
                break result;
            }
            assert!(Instant::now() < deadline, "control round trip timed out");
            std::thread::sleep(Duration::from_millis(20));
        };
        let (role, status, logs, unknown_failed) = result;
        assert_eq!(role, SupervisorRole::Daemon);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].id, id);
        assert_eq!(status[0].status, "stopped");
        assert_eq!(logs.next_seq, 0);
        assert!(unknown_failed);
    }

    #[test]
    fn a_second_daemon_is_refused_and_the_endpoint_is_cleaned_up_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let first = Engine::new_daemon(dir.path()).expect("first daemon");
        // The second daemon pings the first one, which only answers from its own loop.
        let ticker_dir = dir.path().to_path_buf();
        let handle = std::thread::spawn(move || Engine::new_daemon(&ticker_dir).map(|_| ()));
        let mut first = first;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() {
            first.tick();
            assert!(Instant::now() < deadline, "second daemon never got an answer");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(matches!(handle.join().unwrap(), Err(DaemonStartError::AlreadySupervised { .. })));

        drop(first);
        assert!(crate::control::Endpoint::read(dir.path()).is_none());
        assert!(SupervisorClient::discover(dir.path()).is_none());
    }
    #[test]
    fn a_native_app_takes_over_from_the_daemon_and_relaunches_its_apps() {
        let dir = tempfile::tempdir().unwrap();
        let mut daemon = Engine::new_daemon(dir.path()).expect("daemon engine");
        let id = daemon.add_app(AppDraft { command: "while true; do sleep 1; done".to_string(), ..base_draft() });
        assert_eq!(daemon.start_app(id), ActionOutcome::Started);
        let daemon_thread = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !daemon.shutdown_requested() && Instant::now() < deadline {
                daemon.tick();
                std::thread::sleep(Duration::from_millis(20));
            }
            let still_running = daemon.has_active_apps();
            daemon.shutdown(Duration::from_secs(2));
            still_running
        });

        let mut app = Engine::supervising(ConfigFile::in_dir(dir.path()));

        assert!(!daemon_thread.join().unwrap(), "the daemon must stop its processes before handing over");
        assert!(app.has_active_apps(), "the app relaunches what the daemon was running");
        let endpoint = crate::control::Endpoint::read(dir.path()).expect("the app publishes its endpoint");
        assert_eq!(endpoint.role, SupervisorRole::App);
        assert_eq!(endpoint.pid, std::process::id());
        app.shutdown(Duration::from_secs(2));
    }
}


