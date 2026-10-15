mod about_dialog;
mod add_dialog;
mod config_export_dialog;
mod dialog_shell;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use switchboard_core::{AppKind, AppView, Engine};
use gtk::{gdk, gio, glib};
use uuid::Uuid;

const APP_ID: &str = "com.skolln.switchboard";
/// Mirrors the Rust engine's MAX_LOG_LINES cap so a caught-up client's buffer can't grow unbounded.
const MAX_DISPLAYED_LOG_LINES: usize = 5000;

fn status_css_class(view: &AppView) -> &'static str {
    match view.status_label {
        "running" => "status-running",
        "building" => "status-building",
        "failed" => "status-failed",
        _ => "status-stopped",
    }
}

/// Shared by build_row and update_row so both compute the subtitle the same way.
fn row_subtitle(view: &AppView) -> String {
    match (&view.error, view.active, view.healthy) {
        (Some(err), _, _) => err.clone(),
        (None, true, Some(true)) => format!(
            "{} · ✓ healthy · {:.0}% CPU · {:.0} Mo",
            view.status_label, view.cpu_percent, view.memory_mb
        ),
        (None, true, Some(false)) => format!(
            "{} · ✗ ne répond pas · {:.0}% CPU · {:.0} Mo",
            view.status_label, view.cpu_percent, view.memory_mb
        ),
        (None, true, None) => {
            format!("{} · {:.0}% CPU · {:.0} Mo", view.status_label, view.cpu_percent, view.memory_mb)
        }
        (None, false, _) => view.status_label.to_string(),
    }
}

fn kind_label_text(kind: AppKind) -> String {
    kind.display_name().to_uppercase()
}

/// Kept alive across refreshes so update_row can patch a row in place.
struct RowWidgets {
    row: adw::ActionRow,
    dot: gtk::Box,
    kind_label: gtk::Label,
    open_btn: gtk::Button,
    start_btn: gtk::Button,
    stop_btn: gtk::Button,
    /// Lets open_btn/edit_btn always use current data even though the row is reused.
    current_view: Rc<RefCell<AppView>>,
}

fn load_styles() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "
        .status-dot { min-width: 10px; min-height: 10px; border-radius: 6px; margin: 0 4px; }
        .status-stopped { background-color: #8e8e93; }
        .status-building { background-color: #ff9f0a; }
        .status-running { background-color: #30d158; box-shadow: 0 0 6px 2px #30d15880; }
        .status-failed { background-color: #ff453a; }
        .terminal { background-color: #1c1c1e; color: #d6d6d8; font-family: monospace; padding: 10px; }
        ",
    );
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn show_message(parent: &impl IsA<gtk::Window>, text: &str, secondary: &str) {
    gtk::AlertDialog::builder()
        .modal(true)
        .message(text)
        .detail(secondary)
        .buttons(["OK"])
        .build()
        .show(Some(parent));
}

struct Ui {
    app: adw::Application,
    engine: Rc<RefCell<Engine>>,
    list_box: gtk::ListBox,
    log_view: gtk::TextView,
    search_entry: gtk::SearchEntry,
    header_label: adw::WindowTitle,
    selected: Rc<RefCell<Option<Uuid>>>,
    /// Dernier statut connu par app, pour detecter les transitions vers "failed".
    last_status: Rc<RefCell<HashMap<Uuid, &'static str>>>,
    /// Derniere revision vue par le poll — saute le refresh complet si rien n'a change.
    last_seen_revision: Rc<Cell<u64>>,
    /// Sequence du dernier log connu pour l'app selectionnee, envoyee au moteur pour le delta.
    since_seq: Rc<Cell<u64>>,
    /// Logs accumules cote client pour l'app selectionnee (append ou remplacement complet).
    selected_logs: Rc<RefCell<Vec<String>>>,
    /// Widgets de chaque ligne, pour patcher en place plutot que rebuild a chaque tick.
    row_widgets: Rc<RefCell<HashMap<Uuid, RowWidgets>>>,
    /// Ordre des ids au dernier rebuild complet — sert a detecter si on peut patcher.
    last_row_order: Rc<RefCell<Vec<Uuid>>>,
    /// Vers soi-meme, pour que weak_refresh puisse rappeler refresh_now sans dupliquer
    /// tous les champs — pose juste apres le Rc::new dans build_ui.
    self_weak: RefCell<Weak<Ui>>,
}

impl Ui {
    fn refresh_now(&self) {
        let selected = *self.selected.borrow();
        let logs_for = selected.map(|id| (id, self.since_seq.get()));
        let apps = self.engine.borrow_mut().list_apps(logs_for);
        self.notify_new_failures(&apps);

        // Set when render_logs can patch the buffer incrementally instead of a full rebuild.
        let mut incremental_append: Option<(usize, Vec<String>)> = None;
        if let Some(id) = selected
            && let Some(view) = apps.iter().find(|a| a.id == id)
        {
            if view.logs_replace {
                *self.selected_logs.borrow_mut() = view.logs.clone();
            } else if !view.logs.is_empty() {
                let mut logs = self.selected_logs.borrow_mut();
                // Buffer still shows the placeholder if it was empty — needs a full render.
                let was_empty = logs.is_empty();
                logs.extend(view.logs.iter().cloned());
                let overflow = logs.len().saturating_sub(MAX_DISPLAYED_LOG_LINES);
                if overflow > 0 {
                    logs.drain(0..overflow);
                }
                if !was_empty {
                    let survive = view.logs.len().min(logs.len());
                    let appended = logs[logs.len() - survive..].to_vec();
                    incremental_append = Some((overflow, appended));
                }
            }
            self.since_seq.set(view.logs_base_seq + view.logs.len() as u64);
        }

        let mut selected_view: Option<AppView> = None;
        for view in &apps {
            if Some(view.id) == selected {
                selected_view = Some(view.clone());
            }
        }

        let current_order: Vec<Uuid> = apps.iter().map(|v| v.id).collect();
        let order_unchanged = *self.last_row_order.borrow() == current_order;

        if order_unchanged && !self.row_widgets.borrow().is_empty() {
            let widgets = self.row_widgets.borrow();
            for view in &apps {
                if let Some(w) = widgets.get(&view.id) {
                    self.update_row(w, view);
                }
            }
        } else {
            while let Some(child) = self.list_box.first_child() {
                self.list_box.remove(&child);
            }
            let mut widgets = self.row_widgets.borrow_mut();
            widgets.clear();
            for view in &apps {
                let (widget, row_widgets) = self.build_row(view);
                self.list_box.append(&widget);
                widgets.insert(view.id, row_widgets);
            }
            drop(widgets);
            *self.last_row_order.borrow_mut() = current_order;
        }

        if selected_view.is_none()
            && let Some(first) = apps.first()
        {
            // Fresh fallback selection: reset log state, or a stale since_seq from
            // the old app corrupts the new app's log diff on the next refresh.
            *self.selected.borrow_mut() = Some(first.id);
            self.since_seq.set(0);
            self.selected_logs.borrow_mut().clear();
            selected_view = Some(first.clone());
        }

        if let Some(view) = selected_view {
            self.header_label.set_subtitle(&view.name);
            self.render_logs(incremental_append);
        } else {
            self.header_label.set_subtitle("Aucune app configurée");
            self.log_view.buffer().set_text("");
        }
    }

    fn refresh(&self) {
        let rev = self.engine.borrow_mut().revision();
        if rev != self.last_seen_revision.get() {
            self.last_seen_revision.set(rev);
            self.refresh_now();
        }
    }

    fn render_logs(&self, incremental: Option<(usize, Vec<String>)>) {
        let filter = self.search_entry.text().to_string().to_lowercase();
        let buffer = self.log_view.buffer();

        if filter.is_empty()
            && let Some((trimmed, new_lines)) = incremental
            && !new_lines.is_empty()
        {
            // Delete the trimmed lines, insert the new ones — avoids a full set_text.
            if trimmed > 0
                && let Some(mut cut) = buffer.iter_at_line(trimmed as i32)
            {
                buffer.delete(&mut buffer.start_iter(), &mut cut);
            }
            let chunk = format!("\n{}", new_lines.join("\n"));
            buffer.insert(&mut buffer.end_iter(), &chunk);
            let mut end = buffer.end_iter();
            self.log_view.scroll_to_iter(&mut end, 0.0, false, 0.0, 0.0);
            return;
        }

        let logs = self.selected_logs.borrow();
        if logs.is_empty() {
            buffer.set_text("Pas encore de logs. Démarre l'app pour voir sa sortie ici.");
            return;
        }
        let text = if filter.is_empty() {
            logs.join("\n")
        } else {
            // Collect refs, not clones — join() just needs to read them.
            logs.iter()
                .filter(|l| l.to_lowercase().contains(&filter))
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        };
        drop(logs);
        buffer.set_text(&text);
        let mut end = buffer.end_iter();
        self.log_view.scroll_to_iter(&mut end, 0.0, false, 0.0, 0.0);
    }

    fn notify_new_failures(&self, apps: &[AppView]) {
        let mut last = self.last_status.borrow_mut();
        for app in apps {
            let previous = last.insert(app.id, app.status_label);
            if app.status_label == "failed" && previous != Some("failed") {
                let notification = gio::Notification::new(&format!("{} a crashé", app.name));
                if let Some(err) = &app.error {
                    notification.set_body(Some(err));
                }
                notification.set_priority(gio::NotificationPriority::High);
                self.app.send_notification(Some(&format!("crash-{}", app.id)), &notification);
            }
        }
    }

    fn build_row(&self, view: &AppView) -> (gtk::Widget, RowWidgets) {
        let current_view = Rc::new(RefCell::new(view.clone()));

        let row = adw::ActionRow::builder()
            .title(&view.name)
            .subtitle(row_subtitle(view))
            .activatable(true)
            .build();

        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.set_css_classes(&["status-dot", status_css_class(view)]);
        dot.set_valign(gtk::Align::Center);
        dot.set_halign(gtk::Align::Center);
        dot.set_vexpand(false);
        dot.set_size_request(10, 10);
        row.add_prefix(&dot);

        let kind_label = gtk::Label::new(Some(&kind_label_text(view.kind)));
        kind_label.add_css_class("dim-label");
        kind_label.add_css_class("caption");
        row.add_suffix(&kind_label);

        // Always created; visibility toggled in update_row instead of add/remove per refresh.
        let open_btn = gtk::Button::from_icon_name("web-browser-symbolic");
        open_btn.set_valign(gtk::Align::Center);
        open_btn.add_css_class("flat");
        open_btn.set_tooltip_text(Some("Ouvrir dans le navigateur"));
        open_btn.set_visible(view.url.is_some());
        {
            let current_view = current_view.clone();
            open_btn.connect_clicked(move |_| {
                if let Some(url) = current_view.borrow().url.clone() {
                    let _ = gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>);
                }
            });
        }
        row.add_suffix(&open_btn);

        let edit_btn = gtk::Button::from_icon_name("document-edit-symbolic");
        edit_btn.set_valign(gtk::Align::Center);
        edit_btn.add_css_class("flat");
        let start_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
        start_btn.set_valign(gtk::Align::Center);
        start_btn.set_sensitive(!view.active);
        start_btn.add_css_class("flat");
        let stop_btn = gtk::Button::from_icon_name("media-playback-stop-symbolic");
        stop_btn.set_valign(gtk::Align::Center);
        stop_btn.set_sensitive(view.active);
        stop_btn.add_css_class("flat");
        let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        delete_btn.set_valign(gtk::Align::Center);
        delete_btn.add_css_class("flat");

        let id = view.id;
        {
            let engine = self.engine.clone();
            let selected = self.selected.clone();
            let since_seq = self.since_seq.clone();
            let selected_logs = self.selected_logs.clone();
            let this_weak = self.weak_refresh();
            start_btn.connect_clicked(move |_| {
                engine.borrow_mut().start_app(id);
                *selected.borrow_mut() = Some(id);
                since_seq.set(0);
                selected_logs.borrow_mut().clear();
                this_weak();
            });
        }
        {
            let engine = self.engine.clone();
            let this_weak = self.weak_refresh();
            stop_btn.connect_clicked(move |_| {
                engine.borrow_mut().stop_app(id);
                this_weak();
            });
        }
        {
            let engine = self.engine.clone();
            let this_weak = self.weak_refresh();
            delete_btn.connect_clicked(move |_| {
                engine.borrow_mut().remove_app(id);
                this_weak();
            });
        }
        {
            let engine = self.engine.clone();
            let this_weak = self.weak_refresh();
            let row_for_window = row.clone();
            let current_view = current_view.clone();
            edit_btn.connect_clicked(move |_| {
                let Some(window) = row_for_window.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else { return };
                let engine = engine.clone();
                let this_weak = this_weak.clone();
                // Read current data, not the stale snapshot from when this row was built.
                let view_snapshot = current_view.borrow().clone();
                add_dialog::show_app_dialog(&window, Some(&view_snapshot), move |draft| {
                    engine.borrow_mut().update_app(id, draft);
                    this_weak();
                });
            });
        }

        row.add_suffix(&edit_btn);
        row.add_suffix(&start_btn);
        row.add_suffix(&stop_btn);
        row.add_suffix(&delete_btn);

        {
            let selected = self.selected.clone();
            let since_seq = self.since_seq.clone();
            let selected_logs = self.selected_logs.clone();
            let this_weak = self.weak_refresh();
            row.connect_activate(move |_| {
                *selected.borrow_mut() = Some(id);
                since_seq.set(0);
                selected_logs.borrow_mut().clear();
                this_weak();
            });
        }

        let widgets = RowWidgets {
            row: row.clone(),
            dot,
            kind_label,
            open_btn,
            start_btn,
            stop_btn,
            current_view,
        };
        (row.upcast(), widgets)
    }

    /// Must stay in sync with the mutable fields build_row sets.
    fn update_row(&self, w: &RowWidgets, view: &AppView) {
        *w.current_view.borrow_mut() = view.clone();
        w.row.set_title(&view.name);
        w.row.set_subtitle(&row_subtitle(view));
        w.dot.set_css_classes(&["status-dot", status_css_class(view)]);
        w.kind_label.set_label(&kind_label_text(view.kind));
        w.open_btn.set_visible(view.url.is_some());
        w.start_btn.set_sensitive(!view.active);
        w.stop_btn.set_sensitive(view.active);
    }

    /// Rappelle `refresh_now` depuis une closure de callback GTK sans capturer `Rc<Ui>`
    /// directement (garderait l'UI en vie via un cycle de reference).
    fn weak_refresh(&self) -> impl Fn() + Clone + 'static {
        let weak = self.self_weak.borrow().clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                ui.refresh_now();
            }
        }
    }
}

fn build_ui(app: &adw::Application) {
    load_styles();

    let engine = Rc::new(RefCell::new(Engine::new()));

    // GTK closures can outlive build_ui and keep the last Engine Rc alive past window
    // close, so its Drop never fires — stop child processes explicitly here instead.
    {
        let engine = engine.clone();
        app.connect_shutdown(move |_| {
            engine.borrow_mut().stop_all_running();
        });
    }

    let header_label = adw::WindowTitle::new("Switchboard", "");
    let header = adw::HeaderBar::builder().title_widget(&header_label).build();

    let add_btn = gtk::Button::from_icon_name("list-add-symbolic");
    add_btn.set_tooltip_text(Some("Ajouter une app"));
    header.pack_start(&add_btn);

    let start_all_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
    start_all_btn.set_tooltip_text(Some("Tout démarrer"));
    let stop_all_btn = gtk::Button::from_icon_name("media-playback-stop-symbolic");
    stop_all_btn.set_tooltip_text(Some("Tout arrêter"));
    header.pack_start(&start_all_btn);
    header.pack_start(&stop_all_btn);

    let clear_logs_btn = gtk::Button::from_icon_name("edit-clear-all-symbolic");
    clear_logs_btn.set_tooltip_text(Some("Effacer les logs"));
    header.pack_end(&clear_logs_btn);

    let export_logs_btn = gtk::Button::from_icon_name("document-save-symbolic");
    export_logs_btn.set_tooltip_text(Some("Exporter les logs…"));
    header.pack_end(&export_logs_btn);

    let export_config_btn = gtk::Button::from_icon_name("document-send-symbolic");
    export_config_btn.set_tooltip_text(Some("Exporter la config…"));
    header.pack_end(&export_config_btn);

    let import_config_btn = gtk::Button::from_icon_name("document-open-symbolic");
    import_config_btn.set_tooltip_text(Some("Importer une config…"));
    header.pack_end(&import_config_btn);

    let about_btn = gtk::Button::from_icon_name("help-about-symbolic");
    about_btn.set_tooltip_text(Some("À propos de Switchboard"));
    header.pack_end(&about_btn);

    let list_box = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list".to_string()])
        .build();

    let sidebar_scroller = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).build();
    let sidebar_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_box.append(&sidebar_scroller);
    sidebar_box.set_width_request(420);

    let search_entry = gtk::SearchEntry::builder().placeholder_text("Filtrer les logs…").build();

    let log_view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .css_classes(vec!["terminal".to_string()])
        .build();
    let log_scroller = gtk::ScrolledWindow::builder().child(&log_view).hexpand(true).vexpand(true).build();

    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_box.append(&search_entry);
    content_box.append(&log_scroller);

    let split = gtk::Paned::builder().orientation(gtk::Orientation::Horizontal).build();
    split.set_start_child(Some(&sidebar_box));
    split.set_end_child(Some(&content_box));
    split.set_position(420);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&split));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Switchboard")
        .default_width(1080)
        .default_height(620)
        .content(&toolbar_view)
        .build();

    let ui = Rc::new(Ui {
        app: app.clone(),
        engine: engine.clone(),
        list_box,
        log_view,
        search_entry: search_entry.clone(),
        header_label,
        selected: Rc::new(RefCell::new(None)),
        last_status: Rc::new(RefCell::new(HashMap::new())),
        last_seen_revision: Rc::new(Cell::new(0)),
        since_seq: Rc::new(Cell::new(0)),
        selected_logs: Rc::new(RefCell::new(Vec::new())),
        row_widgets: Rc::new(RefCell::new(HashMap::new())),
        last_row_order: Rc::new(RefCell::new(Vec::new())),
        self_weak: RefCell::new(Weak::new()),
    });
    *ui.self_weak.borrow_mut() = Rc::downgrade(&ui);

    {
        let ui = ui.clone();
        let window = window.clone();
        add_btn.connect_clicked(move |_| {
            let engine = ui.engine.clone();
            let this_weak = ui.weak_refresh();
            add_dialog::show_app_dialog(&window, None, move |draft| {
                engine.borrow_mut().add_app(draft);
                this_weak();
            });
        });
    }
    {
        let ui = ui.clone();
        let window = window.clone();
        export_config_btn.connect_clicked(move |_| {
            let apps = ui.engine.borrow_mut().list_apps(None);
            let ui = ui.clone();
            config_export_dialog::show_export_dialog(&window, &apps, move |ids, include_env_vars| {
                ui.engine.borrow().export_config(&ids, include_env_vars)
            });
        });
    }
    {
        let ui = ui.clone();
        let window = window.clone();
        import_config_btn.connect_clicked(move |_| {
            let file_dialog = gtk::FileDialog::builder().title("Importer une config").build();
            let ui = ui.clone();
            let window = window.clone();
            glib::spawn_future_local(async move {
                let Ok(file) = file_dialog.open_future(Some(&window)).await else { return };
                let Some(path) = file.path() else { return };
                let Ok(contents) = std::fs::read_to_string(&path) else {
                    show_message(&window, "Fichier invalide", "Impossible de lire ce fichier.");
                    return;
                };
                let Some(preview) = ui.engine.borrow().preview_import(&contents) else {
                    show_message(&window, "Fichier invalide", "Ce fichier ne contient pas une configuration Switchboard valide.");
                    return;
                };
                if preview.to_add.is_empty() && preview.to_replace.is_empty() {
                    show_message(&window, "Rien à importer", "Ce fichier ne contient aucune app à ajouter ou remplacer.");
                    return;
                }
                let mut detail = String::new();
                if !preview.to_add.is_empty() {
                    detail.push_str(&format!(
                        "{} app(s) seront ajoutées : {}\n",
                        preview.to_add.len(),
                        preview.to_add.join(", ")
                    ));
                }
                if !preview.to_replace.is_empty() {
                    detail.push_str(&format!(
                        "{} app(s) seront remplacées : {}",
                        preview.to_replace.len(),
                        preview.to_replace.join(", ")
                    ));
                }
                let confirm = gtk::AlertDialog::builder()
                    .modal(true)
                    .message("Importer cette configuration ?")
                    .detail(&detail)
                    .buttons(["Annuler", "Importer"])
                    .cancel_button(0)
                    .default_button(1)
                    .build();
                if confirm.choose_future(Some(&window)).await == Ok(1) {
                    ui.engine.borrow_mut().apply_import(&contents);
                    ui.refresh_now();
                }
            });
        });
    }
    {
        let ui = ui.clone();
        start_all_btn.connect_clicked(move |_| {
            ui.engine.borrow_mut().start_all();
            ui.refresh_now();
        });
    }
    {
        let ui = ui.clone();
        stop_all_btn.connect_clicked(move |_| {
            ui.engine.borrow_mut().stop_all_running();
            ui.refresh_now();
        });
    }
    {
        let ui = ui.clone();
        clear_logs_btn.connect_clicked(move |_| {
            if let Some(id) = *ui.selected.borrow() {
                ui.engine.borrow_mut().clear_logs(id);
            }
            ui.since_seq.set(0);
            ui.selected_logs.borrow_mut().clear();
            ui.refresh_now();
        });
    }
    {
        let ui = ui.clone();
        search_entry.connect_search_changed(move |_| ui.refresh_now());
    }
    {
        let ui = ui.clone();
        let window = window.clone();
        export_logs_btn.connect_clicked(move |_| {
            let Some(id) = *ui.selected.borrow() else { return };
            let dialog = gtk::FileDialog::builder()
                .title("Exporter les logs")
                .initial_name(format!("{id}.log"))
                .build();
            let engine = ui.engine.clone();
            let window = window.clone();
            glib::spawn_future_local(async move {
                if let Ok(file) = dialog.save_future(Some(&window)).await
                    && let Some(path) = file.path()
                {
                    let _ = engine.borrow().export_logs(id, &path);
                }
            });
        });
    }
    {
        let window = window.clone();
        about_btn.connect_clicked(move |_| about_dialog::show_about_dialog(&window));
    }

    ui.refresh_now();

    // Poll periodique, meme intervalle que macOS/Windows.
    {
        let ui = ui.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            ui.refresh();
            glib::ControlFlow::Continue
        });
    }

    window.present();
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::empty()).build();
    app.connect_activate(build_ui);
    app.run()
}
