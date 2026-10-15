use serde_json::{json, Value};
use switchboard_core::control::{ControlRequest, DEFAULT_REQUEST_TIMEOUT};
use switchboard_core::{validate_draft, AppConfig, AppConfigList, AppDraft, FieldIssue};
use uuid::Uuid;

use crate::app_json::{app_json, changed_fields, read_json_arg, status_of, FieldPatch};
use crate::app_ref::{resolve, resolve_many};
use crate::args::{AddArgs, EditArgs, ExportArgs, ImportArgs};
use crate::context::{supervisor_json, supervisor_label, Context};
use crate::error::{CliError, ErrorCode};
use crate::output::{table, Output};

pub fn list(ctx: &Context) -> Result<Output, CliError> {
    let config = ctx.load()?;
    let (supervisor, runtimes) = ctx.runtimes()?;
    let apps: Vec<Value> = config.apps.iter().map(|a| app_json(a, runtimes.get(&a.id))).collect();
    let rows: Vec<Vec<String>> = config
        .apps
        .iter()
        .map(|a| {
            vec![
                a.name.clone(),
                status_of(runtimes.get(&a.id)),
                a.kind.ffi_str().to_string(),
                a.id.to_string()[..8].to_string(),
                a.url.clone().unwrap_or_default(),
            ]
        })
        .collect();
    let human = if rows.is_empty() {
        "No apps configured. Add one with `switchboard-cli add --name <NAME> --dir <PATH> --kind <KIND> ...`.".to_string()
    } else {
        format!("{}\n\nsupervisor: {}", table(&["NAME", "STATUS", "KIND", "ID", "URL"], &rows), supervisor_label(supervisor.as_ref()))
    };
    Ok(Output::new(
        json!({ "config_path": ctx.config.path().display().to_string(), "supervisor": supervisor_json(supervisor.as_ref()), "apps": apps }),
        human,
    ))
}

pub fn show(ctx: &Context, reference: &str) -> Result<Output, CliError> {
    let config = ctx.load()?;
    let app = resolve(&config, reference)?;
    let (supervisor, runtimes) = ctx.runtimes()?;
    let runtime = runtimes.get(&app.id);
    let value = app_json(app, runtime);
    Ok(Output::new(json!({ "supervisor": supervisor_json(supervisor.as_ref()), "app": value }), describe(app, &value)))
}

pub fn add(ctx: &Context, args: &AddArgs) -> Result<Output, CliError> {
    let patch = FieldPatch::from_fields(&args.fields, &current_dir()?)?;
    let mut draft = AppDraft::default();
    patch.apply(&mut draft);
    draft.normalize();

    let (app, warnings) = ctx.config.update(|list| {
        let mut report = validate_draft(&draft, list, None);
        if patch.kind.is_none() {
            // Sans type, les regles de la commande n'ont pas de sens.
            report.errors.retain(|e| e.field != "command");
            report.warnings.retain(|w| w.field != "command");
            report.errors.insert(0, issue("kind", "required", "kind is required (cargo, npm, dotnet, maven, python, go, raw)"));
        }
        if !report.is_valid() {
            return Err(validation_error(report.errors, &report.warnings));
        }
        let app = draft.clone().into_config(Uuid::new_v4());
        list.add(app.clone());
        Ok((app, report.warnings))
    })?;
    let value = app_json(&app, None);
    Ok(Output::new(json!({ "app": value }), format!("Added \"{}\" ({})", app.name, app.id)).with_warnings(warnings))
}

pub fn edit(ctx: &Context, args: &EditArgs) -> Result<Output, CliError> {
    let mut patch = FieldPatch::from_fields(&args.fields, &current_dir()?)?;
    if args.no_url {
        patch.url = Some(None);
    }
    if args.clear_env {
        patch.env_vars = Some(Vec::new());
    }
    patch.env_unset.extend(args.unset_env.iter().cloned());

    let (app, changed, warnings) = ctx.config.update(|list| {
        let existing = resolve(list, &args.app)?.clone();
        let before = AppDraft::from_config(&existing);
        let mut draft = before.clone();
        patch.apply(&mut draft);
        draft.normalize();
        let report = validate_draft(&draft, list, Some(existing.id));
        if !report.is_valid() {
            return Err(validation_error(report.errors, &report.warnings));
        }
        let changed = changed_fields(&before, &draft);
        let app = draft.into_config(existing.id);
        list.update(existing.id, |slot| *slot = app.clone());
        Ok((app, changed, report.warnings))
    })?;

    let (_, runtimes) = ctx.runtimes()?;
    let runtime = runtimes.get(&app.id);
    let restart_required = !changed.is_empty() && runtime.is_some_and(|rt| rt.active);
    let mut human = if changed.is_empty() {
        format!("\"{}\" unchanged", app.name)
    } else {
        format!("Updated \"{}\": {}", app.name, changed.join(", "))
    };
    if restart_required {
        human.push_str(&format!("\nThe app is running: `switchboard-cli restart {}` applies the change.", app.id));
    }
    Ok(Output::new(
        json!({ "app": app_json(&app, runtime), "changed": changed, "restart_required": restart_required }),
        human,
    )
    .with_warnings(warnings))
}

pub fn remove(ctx: &Context, references: &[String]) -> Result<Output, CliError> {
    let targets = resolve_many(&ctx.load()?, references)?;
    let ids: Vec<Uuid> = targets.iter().map(|a| a.id).collect();
    // Arret explicite (et attendu) plutot que de compter sur le rechargement du superviseur.
    if let Some(client) = ctx.supervisor() {
        client.request(&ControlRequest::Stop { ids: ids.clone() }, DEFAULT_REQUEST_TIMEOUT)?;
    }
    let removed = ctx.config.update(|list| {
        let removed: Vec<AppConfig> = list.apps.iter().filter(|a| ids.contains(&a.id)).cloned().collect();
        list.apps.retain(|a| !ids.contains(&a.id));
        Ok::<_, CliError>(removed)
    })?;
    let names: Vec<&str> = removed.iter().map(|a| a.name.as_str()).collect();
    Ok(Output::new(
        json!({ "removed": removed.iter().map(|a| json!({ "id": a.id, "name": a.name })).collect::<Vec<_>>() }),
        format!("Removed {}", names.join(", ")),
    ))
}

pub fn export(ctx: &Context, args: &ExportArgs) -> Result<Output, CliError> {
    let config = ctx.load()?;
    let ids: Vec<Uuid> = if args.apps.is_empty() {
        config.apps.iter().map(|a| a.id).collect()
    } else {
        resolve_many(&config, &args.apps)?.iter().map(|a| a.id).collect()
    };
    let subset = config.export_subset(&ids, args.include_env);
    let text = subset.to_json().map_err(|e| CliError::new(ErrorCode::Internal, e.to_string()))?;
    let config_value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    match &args.output {
        Some(path) => {
            std::fs::write(path, format!("{text}\n")).map_err(|e| CliError::io(path.display(), e))?;
            let path = path.display().to_string();
            Ok(Output::new(
                json!({ "path": path, "count": subset.apps.len(), "include_env": args.include_env, "config": config_value }),
                format!("Exported {} app(s) to {path}", subset.apps.len()),
            ))
        }
        None => Ok(Output::new(
            json!({ "path": null, "count": subset.apps.len(), "include_env": args.include_env, "config": config_value }),
            text,
        )),
    }
}

pub fn import(ctx: &Context, args: &ImportArgs) -> Result<Output, CliError> {
    let source = if args.file == "-" { "-".to_string() } else { format!("@{}", args.file) };
    let value = read_json_arg(&source)?;
    let mut incoming: AppConfigList = serde_json::from_value(value).map_err(|e| {
        CliError::invalid_input(format!("not a Switchboard config: {e}"))
            .with_hint("expected {\"apps\": [...]} as written by `switchboard-cli export` or the native export")
    })?;
    if !args.skip_validation {
        validate_import(&mut incoming)?;
    }

    let preview = ctx.load()?.merge_import(incoming.clone());
    if args.dry_run {
        return Ok(import_output(&preview, true));
    }
    let summary = ctx.config.update(|list| Ok::<_, CliError>(list.merge_import(incoming)))?;
    Ok(import_output(&summary, false))
}

/// Normalise comme le formulaire et refuse tout le fichier au premier probleme : rien n'est ecrit.
fn validate_import(incoming: &mut AppConfigList) -> Result<(), CliError> {
    let mut problems = Vec::new();
    let mut seen = AppConfigList::default();
    for app in &mut incoming.apps {
        let mut draft = AppDraft::from_config(app);
        draft.normalize();
        let report = validate_draft(&draft, &seen, None);
        for error in report.errors {
            problems.push(json!({ "app": draft.name, "field": error.field, "code": error.code, "message": error.message }));
        }
        *app = draft.into_config(app.id);
        seen.add(app.clone());
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(CliError::new(ErrorCode::ValidationFailed, format!("{} problem(s) in the import file; nothing was imported", problems.len()))
        .with_hint("fix the file, or pass --skip-validation to import it like the native apps do")
        .with_details(json!({ "errors": problems })))
}

fn import_output(summary: &switchboard_core::ImportSummary, dry_run: bool) -> Output {
    let verb = if dry_run { "Would add" } else { "Added" };
    let replace_verb = if dry_run { "would replace" } else { "replaced" };
    Output::new(
        json!({ "dry_run": dry_run, "added": summary.to_add, "replaced": summary.to_replace, "invalid": summary.invalid }),
        format!(
            "{verb} {} app(s), {replace_verb} {}{}",
            summary.to_add.len(),
            summary.to_replace.len(),
            if summary.invalid > 0 { format!(", skipped {} without a name", summary.invalid) } else { String::new() }
        ),
    )
}

fn describe(app: &AppConfig, value: &Value) -> String {
    let runtime = &value["runtime"];
    let mut lines = vec![
        format!("{} ({})", app.name, app.id),
        format!("  kind          {}", app.kind.ffi_str()),
        format!("  working_dir   {}", app.working_dir.display()),
        format!("  command       {}", app.command),
        format!("  launch        {}", value["launch"].as_array().map(|l| l.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" && ")).unwrap_or_default()),
        format!("  url           {}", app.url.as_deref().unwrap_or("-")),
        format!("  auto_restart  {}", app.auto_restart),
        format!("  start_order   {}", app.start_order),
        format!("  status        {}", runtime["status"].as_str().unwrap_or("stopped")),
    ];
    if let Some(error) = runtime["error"].as_str() {
        lines.push(format!("  error         {error}"));
    }
    if let Some(healthy) = runtime["healthy"].as_bool() {
        lines.push(format!("  healthy       {healthy}"));
    }
    if !app.env_vars.is_empty() {
        lines.push("  env_vars".to_string());
        lines.extend(app.env_vars.iter().map(|(k, v)| format!("    {k}={v}")));
    }
    lines.join("\n")
}

fn validation_error(errors: Vec<FieldIssue>, warnings: &[FieldIssue]) -> CliError {
    let summary = errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("; ");
    CliError::new(ErrorCode::ValidationFailed, format!("invalid app: {summary}"))
        .with_hint("run `switchboard-cli schema` for the field rules")
        .with_details(json!({ "errors": errors, "warnings": warnings }))
}

fn issue(field: &'static str, code: &'static str, message: &str) -> FieldIssue {
    FieldIssue { field, code, message: message.to_string() }
}

fn current_dir() -> Result<std::path::PathBuf, CliError> {
    std::env::current_dir().map_err(|e| CliError::io("current directory", e).with_hint("the current directory is needed to resolve relative paths"))
}
