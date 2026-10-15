use adw::prelude::*;
use gtk::glib;
use std::cell::Cell;
use std::rc::Rc;

use switchboard_core::{AppDraft, AppKind, AppView, DraftReport, FieldIssue};

use crate::dialog_shell::dialog_shell;
use crate::issue_text::issue_text;

fn kind_to_index(kind: Option<AppKind>) -> u32 {
    AppKind::ALL.iter().position(|k| Some(*k) == kind).unwrap_or(0) as u32
}

/// `existing` preremplit les champs en mode edition ; `on_save` recoit le draft au clic "Enregistrer",
/// une fois `validate` sans erreur (les avertissements demandent un second clic).
pub fn show_app_dialog(
    parent: &impl IsA<gtk::Window>,
    existing: Option<&AppView>,
    validate: impl Fn(&AppDraft) -> DraftReport + 'static,
    on_save: impl Fn(AppDraft) + 'static,
) {
    let title = if existing.is_some() { "Modifier l'app" } else { "Ajouter une app" };

    let name_entry = gtk::Entry::builder().text(existing.map(|a| a.name.as_str()).unwrap_or("")).build();
    let dir_entry = gtk::Entry::builder()
        .text(existing.map(|a| a.working_dir.as_str()).unwrap_or(""))
        .hexpand(true)
        .build();
    let browse_dir_btn = gtk::Button::with_label("Parcourir…");
    let dir_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    dir_box.append(&dir_entry);
    dir_box.append(&browse_dir_btn);
    let url_entry = gtk::Entry::builder()
        .text(existing.and_then(|a| a.url.as_deref()).unwrap_or(""))
        .placeholder_text("http://localhost:3000 (optionnel)")
        .build();
    let command_entry = gtk::Entry::builder().text(existing.map(|a| a.command.as_str()).unwrap_or("")).build();

    let kind_labels = AppKind::ALL.map(|k| k.display_name());
    let kind_dropdown = gtk::DropDown::builder()
        .model(&gtk::StringList::new(&kind_labels))
        .build();
    kind_dropdown.set_selected(kind_to_index(existing.map(|a| a.kind)));

    let auto_restart_switch = gtk::Switch::builder()
        .active(existing.map(|a| a.auto_restart).unwrap_or(false))
        .valign(gtk::Align::Center)
        .build();

    let start_order_spin = gtk::SpinButton::with_range(0.0, 99.0, 1.0);
    start_order_spin.set_value(existing.map(|a| a.start_order).unwrap_or(0) as f64);

    let env_vars_text = existing
        .map(|a| a.env_vars.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    let env_buffer = gtk::TextBuffer::builder().text(&env_vars_text).build();
    let env_view = gtk::TextView::builder().buffer(&env_buffer).monospace(true).build();
    let env_scroller = gtk::ScrolledWindow::builder()
        .child(&env_view)
        .height_request(80)
        .css_classes(vec!["card".to_string()])
        .build();

    let subtitle = if existing.is_some() {
        "Mets à jour la configuration de cette app"
    } else {
        "Configure une nouvelle app à superviser"
    };

    for widget in [
        name_entry.clone().upcast::<gtk::Widget>(),
        dir_box.clone().upcast::<gtk::Widget>(),
        kind_dropdown.clone().upcast::<gtk::Widget>(),
        command_entry.clone().upcast::<gtk::Widget>(),
        url_entry.clone().upcast::<gtk::Widget>(),
    ] {
        widget.set_hexpand(true);
        widget.set_valign(gtk::Align::Center);
    }

    let general_group = adw::PreferencesGroup::builder().title("Général").build();
    let name_row = adw::ActionRow::builder().title("Nom").build();
    name_row.add_suffix(&name_entry);
    general_group.add(&name_row);
    let dir_row = adw::ActionRow::builder().title("Dossier").build();
    dir_row.add_suffix(&dir_box);
    general_group.add(&dir_row);
    let type_row = adw::ActionRow::builder().title("Type").build();
    type_row.add_suffix(&kind_dropdown);
    general_group.add(&type_row);
    let command_row = adw::ActionRow::builder().title("Commande").build();
    command_row.add_suffix(&command_entry);
    general_group.add(&command_row);

    let exec_group = adw::PreferencesGroup::builder().title("Exécution").build();
    let url_row = adw::ActionRow::builder().title("URL").build();
    url_row.add_suffix(&url_entry);
    exec_group.add(&url_row);
    let auto_restart_row = adw::ActionRow::builder().title("Auto-restart").build();
    auto_restart_switch.set_valign(gtk::Align::Center);
    auto_restart_row.add_suffix(&auto_restart_switch);
    exec_group.add(&auto_restart_row);
    let start_order_row = adw::ActionRow::builder().title("Ordre de démarrage").build();
    start_order_spin.set_valign(gtk::Align::Center);
    start_order_row.add_suffix(&start_order_spin);
    exec_group.add(&start_order_row);

    let advanced_group = adw::PreferencesGroup::builder()
        .title("Avancé")
        .description("Variables d'env — une par ligne, format CLE=valeur")
        .build();
    advanced_group.add(&env_scroller);

    let errors_label = gtk::Label::builder().wrap(true).xalign(0.0).css_classes(vec!["error".to_string()]).build();
    let warnings_label = gtk::Label::builder().wrap(true).xalign(0.0).css_classes(vec!["warning".to_string()]).build();
    let issues_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    issues_box.append(&errors_label);
    issues_box.append(&warnings_label);
    let issues_group = adw::PreferencesGroup::new();
    issues_group.add(&issues_box);
    issues_group.set_visible(false);

    let page = adw::PreferencesPage::new();
    page.add(&issues_group);
    page.add(&general_group);
    page.add(&exec_group);
    page.add(&advanced_group);

    let header_title = adw::WindowTitle::new(title, subtitle);
    let save_label = if existing.is_some() { "Enregistrer" } else { "Ajouter" };
    let (dialog, save_btn) = dialog_shell(parent, title, Some(&header_title), save_label, (480, 560), &page);

    {
        let dialog = dialog.clone();
        let dir_entry = dir_entry.clone();
        browse_dir_btn.connect_clicked(move |_| {
            let file_dialog = gtk::FileDialog::builder().title("Choisir le dossier").build();
            let dir_entry = dir_entry.clone();
            let dialog = dialog.clone();
            glib::spawn_future_local(async move {
                if let Ok(folder) = file_dialog.select_folder_future(Some(&dialog)).await
                    && let Some(path) = folder.path()
                {
                    dir_entry.set_text(&path.to_string_lossy());
                }
            });
        });
    }
    // Avertissements deja montres pour ce contenu : le clic suivant enregistre.
    let warned = Rc::new(Cell::new(false));
    {
        let dialog = dialog.clone();
        save_btn.connect_clicked(move |save_btn| {
            let env_vars = env_buffer
                .text(&env_buffer.start_iter(), &env_buffer.end_iter(), false)
                .lines()
                .filter_map(|line| line.split_once('='))
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .filter(|(k, _)| !k.is_empty())
                .collect::<Vec<_>>();

            let url = url_entry.text().trim().to_string();
            let draft = AppDraft {
                name: name_entry.text().trim().to_string(),
                working_dir: dir_entry.text().trim().into(),
                kind: AppKind::ALL
                    .get(kind_dropdown.selected() as usize)
                    .copied()
                    .unwrap_or(AppKind::Cargo),
                command: command_entry.text().trim().to_string(),
                url: if url.is_empty() { None } else { Some(url) },
                env_vars,
                auto_restart: auto_restart_switch.is_active(),
                start_order: start_order_spin.value() as i32,
            };
            let report = validate(&draft);
            let warnings: Vec<_> = report.warnings.iter().filter(|w| w.code != "ignored").collect();
            errors_label.set_label(&bullet_list(report.errors.iter()));
            errors_label.set_visible(!report.errors.is_empty());
            warnings_label.set_label(&bullet_list(warnings.iter().copied()));
            warnings_label.set_visible(!warnings.is_empty());
            issues_group.set_visible(!report.errors.is_empty() || !warnings.is_empty());
            if !report.errors.is_empty() {
                warned.set(false);
                return;
            }
            if !warnings.is_empty() && !warned.get() {
                warned.set(true);
                save_btn.set_label("Enregistrer quand même");
                return;
            }
            on_save(draft);
            dialog.close();
        });
    }

    dialog.present();
}

fn bullet_list<'a>(issues: impl Iterator<Item = &'a FieldIssue>) -> String {
    issues.map(|issue| format!("• {}", issue_text(issue))).collect::<Vec<_>>().join("\n")
}
