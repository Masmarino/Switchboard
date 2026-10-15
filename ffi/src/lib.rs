//! Shim C ABI pour le moteur `switchboard-core`, consomme par les frontends natifs
//! macOS (Swift) et Windows (C#). Tout passe en JSON plutot qu'en marshaling de structs.

use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;
use std::str::FromStr;

use switchboard_core::{AppDraft, AppKind, Engine, ImportSummary};
use serde::Deserialize;
use uuid::Uuid;

/// # Safety
/// `ptr` doit etre un pointeur C valide vers une chaine NUL-terminee, ou NULL.
unsafe fn c_str_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().ok().map(str::to_owned)
}

/// # Safety
/// `ptr` doit etre un pointeur C valide vers une chaine NUL-terminee, ou NULL.
unsafe fn parse_uuid_arg(ptr: *const c_char) -> Option<Uuid> {
    (unsafe { c_str_to_string(ptr) }).and_then(|s| Uuid::from_str(&s).ok())
}

fn string_to_c(s: String) -> *mut c_char {
    CString::new(s).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

/// Chaines UUID JSON (`["uuid1","uuid2"]`) en `Vec<Uuid>` ; entrees invalides ignorees.
fn parse_id_list(json: &str) -> Vec<Uuid> {
    serde_json::from_str::<Vec<String>>(json)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| Uuid::from_str(s).ok())
        .collect()
}

/// Miroir JSON de `AppDraft` — c'est le format echange par `switchboard_engine_add_app_json`
/// et `switchboard_engine_update_app_json`.
#[derive(Debug, Deserialize)]
struct FfiAppDraft {
    name: String,
    working_dir: String,
    kind: String,
    command: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    env_vars: Vec<(String, String)>,
    #[serde(default)]
    auto_restart: bool,
    #[serde(default)]
    start_order: i32,
}

impl From<FfiAppDraft> for AppDraft {
    fn from(d: FfiAppDraft) -> Self {
        let kind = AppKind::from_ffi_str(&d.kind).unwrap_or(AppKind::Raw);
        AppDraft {
            name: d.name,
            working_dir: PathBuf::from(d.working_dir),
            kind,
            command: d.command,
            url: d.url,
            env_vars: d.env_vars,
            auto_restart: d.auto_restart,
            start_order: d.start_order,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn switchboard_engine_new() -> *mut Engine {
    Box::into_raw(Box::new(Engine::new()))
}

/// # Safety
/// `engine` doit provenir de [`switchboard_engine_new`] et ne doit plus etre utilise apres cet appel.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_free(engine: *mut Engine) {
    if !engine.is_null() {
        drop(unsafe { Box::from_raw(engine) });
    }
}

/// Liste des apps en JSON — seule `selected_id` recoit ses logs (lignes posterieures
/// a `since_seq`, sauf remplacement complet via `logs_replace`). A liberer avec
/// [`switchboard_string_free`].
///
/// # Safety
/// `engine` doit etre un pointeur valide retourne par [`switchboard_engine_new`].
/// `selected_id`, si non-NULL, doit etre une chaine C valide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_list_apps_json(
    engine: *mut Engine,
    selected_id: *const c_char,
    since_seq: u64,
) -> *mut c_char {
    let engine = unsafe { &mut *engine };
    let logs_for = (unsafe { parse_uuid_arg(selected_id) }).map(|id| (id, since_seq));
    let apps = engine.list_apps(logs_for);
    string_to_c(serde_json::to_string(&apps).unwrap_or_else(|_| "[]".to_string()))
}

/// Renvoie la revision courante du moteur (compteur qui n'avance que si quelque
/// chose a reellement change). Bon marche : pas de JSON, pas d'allocation.
///
/// # Safety
/// `engine` doit etre un pointeur valide retourne par [`switchboard_engine_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_revision(engine: *mut Engine) -> u64 {
    let engine = unsafe { &mut *engine };
    engine.revision()
}

/// `draft_json` doit correspondre a [`FfiAppDraft`] (cf. doc du module).
///
/// # Safety
/// `engine` doit etre un pointeur valide. `draft_json` doit etre une chaine C valide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_add_app_json(engine: *mut Engine, draft_json: *const c_char) {
    let engine = unsafe { &mut *engine };
    let Some(json) = (unsafe { c_str_to_string(draft_json) }) else { return };
    if let Ok(draft) = serde_json::from_str::<FfiAppDraft>(&json) {
        engine.add_app(draft.into());
    }
}

/// # Safety
/// `engine` doit etre un pointeur valide. `id` et `draft_json` doivent etre des
/// chaines C valides ; `draft_json` doit correspondre a [`FfiAppDraft`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_update_app_json(
    engine: *mut Engine,
    id: *const c_char,
    draft_json: *const c_char,
) {
    let engine = unsafe { &mut *engine };
    let Some(id) = (unsafe { parse_uuid_arg(id) }) else { return };
    let Some(json) = (unsafe { c_str_to_string(draft_json) }) else { return };
    if let Ok(draft) = serde_json::from_str::<FfiAppDraft>(&json) {
        engine.update_app(id, draft.into());
    }
}

/// Valide un draft avec les memes regles que le CLI. Retourne
/// `{"errors":[{field,code,message}],"warnings":[...]}` (listes vides si tout va bien),
/// a liberer avec [`switchboard_string_free`]. `editing_id` (ou NULL) exclut l'app modifiee
/// du controle d'unicite du nom.
///
/// # Safety
/// `engine` doit etre un pointeur valide. `draft_json` doit etre une chaine C valide ;
/// `editing_id`, si non-NULL, aussi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_validate_draft_json(
    engine: *mut Engine,
    editing_id: *const c_char,
    draft_json: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let editing = unsafe { parse_uuid_arg(editing_id) };
    let report = match (unsafe { c_str_to_string(draft_json) }).map(|json| serde_json::from_str::<FfiAppDraft>(&json)) {
        Some(Ok(draft)) => engine.validate(&draft.into(), editing),
        _ => return std::ptr::null_mut(),
    };
    string_to_c(serde_json::to_string(&report).unwrap_or_else(|_| "{\"errors\":[],\"warnings\":[]}".to_string()))
}

/// Arrete toutes les apps et attend leur fin (SIGKILL apres quelques secondes), puis ferme
/// le canal du CLI. A appeler a la fermeture de l'app, avant [`switchboard_engine_free`].
///
/// # Safety
/// `engine` doit etre un pointeur valide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_shutdown(engine: *mut Engine) {
    let engine = unsafe { &mut *engine };
    engine.shutdown(std::time::Duration::from_secs(3));
}

/// # Safety
/// `engine` doit etre un pointeur valide. `id` doit etre une chaine C valide (UUID).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_remove_app(engine: *mut Engine, id: *const c_char) {
    let engine = unsafe { &mut *engine };
    if let Some(id) = unsafe { parse_uuid_arg(id) } {
        engine.remove_app(id);
    }
}

/// # Safety
/// `engine` doit etre un pointeur valide. `id` doit etre une chaine C valide (UUID).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_start_app(engine: *mut Engine, id: *const c_char) {
    let engine = unsafe { &mut *engine };
    if let Some(id) = unsafe { parse_uuid_arg(id) } {
        engine.start_app(id);
    }
}

/// # Safety
/// `engine` doit etre un pointeur valide. `id` doit etre une chaine C valide (UUID).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_stop_app(engine: *mut Engine, id: *const c_char) {
    let engine = unsafe { &mut *engine };
    if let Some(id) = unsafe { parse_uuid_arg(id) } {
        engine.stop_app(id);
    }
}

/// # Safety
/// `engine` doit etre un pointeur valide. `id` doit etre une chaine C valide (UUID).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_clear_logs(engine: *mut Engine, id: *const c_char) {
    let engine = unsafe { &mut *engine };
    if let Some(id) = unsafe { parse_uuid_arg(id) } {
        engine.clear_logs(id);
    }
}

/// Exporte les logs courants d'une app vers un fichier. Retourne `true` en cas de succes.
///
/// # Safety
/// `engine` doit etre un pointeur valide. `id` et `path` doivent etre des chaines C valides.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_export_logs(
    engine: *mut Engine,
    id: *const c_char,
    path: *const c_char,
) -> bool {
    let engine = unsafe { &*engine };
    let Some(id) = (unsafe { parse_uuid_arg(id) }) else { return false };
    let Some(path) = (unsafe { c_str_to_string(path) }) else { return false };
    engine.export_logs(id, std::path::Path::new(&path)).is_ok()
}

/// # Safety
/// `engine` doit etre un pointeur valide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_start_all(engine: *mut Engine) {
    let engine = unsafe { &mut *engine };
    engine.start_all();
}

/// # Safety
/// `engine` doit etre un pointeur valide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_stop_all(engine: *mut Engine) {
    let engine = unsafe { &mut *engine };
    engine.stop_all_running();
}

/// Sous-ensemble d'apps en JSON (`ids_json` = tableau de chaines UUID), a liberer
/// avec [`switchboard_string_free`].
///
/// # Safety
/// `engine` doit etre un pointeur valide. `ids_json` doit etre une chaine C valide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_export_config_json(
    engine: *mut Engine,
    ids_json: *const c_char,
    include_env_vars: bool,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let Some(ids_json) = (unsafe { c_str_to_string(ids_json) }) else { return std::ptr::null_mut() };
    let ids = parse_id_list(&ids_json);
    string_to_c(engine.export_config(&ids, include_env_vars))
}

/// Calcule l'apercu de fusion (compte + noms) sans rien modifier. Retourne NULL si
/// `config_json` n'est pas un fichier de config Switchboard valide.
///
/// # Safety
/// `engine` doit etre un pointeur valide. `config_json` doit etre une chaine C valide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_preview_import_json(
    engine: *mut Engine,
    config_json: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let Some(json) = (unsafe { c_str_to_string(config_json) }) else { return std::ptr::null_mut() };
    match engine.preview_import(&json) {
        Some(summary) => string_to_c(serialize_summary(&summary)),
        None => std::ptr::null_mut(),
    }
}

/// Applique reellement la fusion et persiste. Retourne NULL si `config_json` n'est
/// pas un fichier de config Switchboard valide (rien n'est modifie dans ce cas).
///
/// # Safety
/// `engine` doit etre un pointeur valide. `config_json` doit etre une chaine C valide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_engine_apply_import_json(
    engine: *mut Engine,
    config_json: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &mut *engine };
    let Some(json) = (unsafe { c_str_to_string(config_json) }) else { return std::ptr::null_mut() };
    match engine.apply_import(&json) {
        Some(summary) => string_to_c(serialize_summary(&summary)),
        None => std::ptr::null_mut(),
    }
}

fn serialize_summary(summary: &ImportSummary) -> String {
    serde_json::to_string(summary).unwrap_or_else(|_| "{\"to_add\":[],\"to_replace\":[],\"invalid\":0}".to_string())
}

/// Libere une chaine retournee par une fonction de ce module.
///
/// # Safety
/// `s` doit provenir d'une fonction de ce module (ou etre NULL), et ne doit plus
/// etre utilise apres cet appel.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switchboard_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take_string(ptr: *mut c_char) -> String {
        assert!(!ptr.is_null());
        let text = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
        unsafe { switchboard_string_free(ptr) };
        text
    }

    #[test]
    fn validate_draft_json_reports_field_errors() {
        let dir = std::env::temp_dir().join(format!("sb-ffi-{}", Uuid::new_v4()));
        let mut engine = Engine::with_config_file(switchboard_core::ConfigFile::in_dir(&dir));
        let draft = CString::new(r#"{"name":"  ","working_dir":"relative","kind":"npm","command":""}"#).unwrap();
        let report: serde_json::Value = serde_json::from_str(&take_string(unsafe {
            switchboard_engine_validate_draft_json(&mut engine, std::ptr::null(), draft.as_ptr())
        }))
        .unwrap();
        let fields: Vec<&str> = report["errors"].as_array().unwrap().iter().map(|e| e["field"].as_str().unwrap()).collect();
        assert_eq!(fields, vec!["name", "working_dir", "command"]);

        let invalid_json = CString::new("not json").unwrap();
        assert!(unsafe { switchboard_engine_validate_draft_json(&mut engine, std::ptr::null(), invalid_json.as_ptr()) }.is_null());
    }
}
