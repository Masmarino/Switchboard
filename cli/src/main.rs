//! `switchboard-cli` : gerer et piloter les apps de lancement de Switchboard depuis un
//! terminal ou un agent IA. Voir docs/cli.md.

mod app_json;
mod app_ref;
mod args;
mod commands;
mod context;
mod daemon_spawn;
mod error;
mod output;
mod schema;

use clap::Parser;

use crate::args::{Cli, Command, DaemonAction};
use crate::commands::{control, daemon, manage};
use crate::context::Context;
use crate::error::CliError;
use crate::output::{print_error, print_success, Output};

fn main() {
    let raw_args: Vec<String> = std::env::args().collect();
    let cli = match Cli::try_parse_from(&raw_args) {
        Ok(cli) => cli,
        Err(e) => std::process::exit(report_parse_error(e, &raw_args)),
    };
    let name = command_name(&cli.command);
    let json_mode = cli.json;
    let result = Context::new(cli.config_dir, json_mode).and_then(|ctx| run(&ctx, &cli.command));
    let code = match result {
        Ok(Some(output)) => {
            print_success(name, &output, json_mode);
            0
        }
        Ok(None) => 0,
        Err(error) => {
            print_error(name, &error, json_mode);
            error.code.exit_code()
        }
    };
    std::process::exit(code);
}

/// `None` quand la commande a deja ecrit sa sortie (logs --follow).
fn run(ctx: &Context, command: &Command) -> Result<Option<Output>, CliError> {
    let output = match command {
        Command::List => manage::list(ctx)?,
        Command::Show { app } => manage::show(ctx, app)?,
        Command::Add(args) => manage::add(ctx, args)?,
        Command::Edit(args) => manage::edit(ctx, args)?,
        Command::Remove { apps } => manage::remove(ctx, apps)?,
        Command::Export(args) => manage::export(ctx, args)?,
        Command::Import(args) => manage::import(ctx, args)?,
        Command::Schema => {
            let path = ctx.config.path().display().to_string();
            Output::new(schema::schema_json(&path), schema::schema_human(&path))
        }
        Command::Info => daemon::info(ctx)?,
        Command::Start(args) => control::start(ctx, args)?,
        Command::Stop(args) => control::stop(ctx, args)?,
        Command::Restart(args) => control::restart(ctx, args)?,
        Command::Status { apps } => control::status(ctx, apps)?,
        Command::Logs(args) => return control::logs(ctx, args),
        Command::Wait(args) => control::wait(ctx, args)?,
        Command::Daemon { action } => match action {
            DaemonAction::Run { idle_timeout } => daemon::run(ctx, *idle_timeout)?,
            DaemonAction::Start => daemon::start(ctx)?,
            DaemonAction::Stop => daemon::stop(ctx)?,
            DaemonAction::Status => daemon::status(ctx)?,
        },
    };
    Ok(Some(output))
}

fn command_name(command: &Command) -> &'static str {
    match command {
        Command::List => "list",
        Command::Show { .. } => "show",
        Command::Add(_) => "add",
        Command::Edit(_) => "edit",
        Command::Remove { .. } => "remove",
        Command::Export(_) => "export",
        Command::Import(_) => "import",
        Command::Schema => "schema",
        Command::Info => "info",
        Command::Start(_) => "start",
        Command::Stop(_) => "stop",
        Command::Restart(_) => "restart",
        Command::Status { .. } => "status",
        Command::Logs(_) => "logs",
        Command::Wait(_) => "wait",
        Command::Daemon { action } => match action {
            DaemonAction::Run { .. } => "daemon run",
            DaemonAction::Start => "daemon start",
            DaemonAction::Stop => "daemon stop",
            DaemonAction::Status => "daemon status",
        },
    }
}

/// `--help`/`--version` restent du texte ; les erreurs d'usage deviennent du JSON avec `--json`.
fn report_parse_error(error: clap::Error, raw_args: &[String]) -> i32 {
    use clap::error::ErrorKind;
    if matches!(error.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand) {
        let _ = error.print();
        return if error.kind() == ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand { 2 } else { 0 };
    }
    let json_mode = raw_args.iter().any(|a| a == "--json")
        || std::env::var("SWITCHBOARD_JSON").is_ok_and(|v| !v.is_empty() && v != "0" && v != "false");
    if !json_mode {
        let _ = error.print();
        return 2;
    }
    let message = error.render().to_string();
    let first_line = message.lines().next().unwrap_or("invalid arguments").trim_start_matches("error: ").to_string();
    let cli_error = CliError::usage(first_line)
        .with_hint("run with --help for the full usage")
        .with_details(serde_json::json!({ "clap": message }));
    let command = raw_args.get(1).map(String::as_str).unwrap_or("");
    print_error(command, &cli_error, true);
    2
}
