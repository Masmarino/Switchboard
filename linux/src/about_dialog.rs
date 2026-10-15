use adw::prelude::*;

const APP_ID: &str = "com.skolln.switchboard";

struct AboutLink {
    icon: &'static str,
    title: &'static str,
    subtitle: &'static str,
    uri: &'static str,
}

const LINKS: &[AboutLink] = &[
    AboutLink {
        icon: "network-server-symbolic",
        title: "Développé par SkollN",
        subtitle: "skolln.com",
        uri: "https://www.skolln.com",
    },
    AboutLink {
        icon: "starred-symbolic",
        title: "Découvre aussi Alume",
        subtitle: "Agrégateur de contenus avec IA intégrée",
        uri: "https://alume.skolln.com",
    },
    AboutLink {
        icon: "text-x-generic-symbolic",
        title: "Code source",
        subtitle: "Open source sous licence GPLv3",
        uri: "https://github.com/masmarino/switchboard",
    },
];

/// Meme langage visuel que `add_dialog` plutot que le `gtk::AboutDialog` generique.
pub fn show_about_dialog(parent: &impl IsA<gtk::Window>) {
    let icon = gtk::Image::from_icon_name(APP_ID);
    icon.set_pixel_size(64);

    let name_label = gtk::Label::builder().label("Switchboard").css_classes(vec!["title-2".to_string()]).build();
    let version_label =
        gtk::Label::builder().label("Version 0.5.0").css_classes(vec!["dim-label".to_string()]).build();

    let header_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    header_box.set_halign(gtk::Align::Center);
    header_box.append(&icon);
    header_box.append(&name_label);
    header_box.append(&version_label);

    let intro_label = gtk::Label::builder()
        .label("Démarre, supervise et orchestre tes process de dev locaux — quel que soit le langage.")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .css_classes(vec!["dim-label".to_string()])
        .build();

    let links_group = adw::PreferencesGroup::builder().title("Liens").build();
    for link in LINKS {
        let row = adw::ActionRow::builder()
            .title(link.title)
            .subtitle(link.subtitle)
            .activatable(true)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name(link.icon));
        row.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
        let uri = link.uri.to_string();
        row.connect_activated(move |_| {
            gtk::UriLauncher::new(&uri).launch(None::<&gtk::Window>, gtk::gio::Cancellable::NONE, |_| {});
        });
        links_group.add(&row);
    }

    let page = adw::PreferencesPage::new();
    page.add(&links_group);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_top(4);
    content.append(&header_box);
    content.append(&intro_label);
    content.append(&page);

    let header = adw::HeaderBar::builder().title_widget(&adw::WindowTitle::new("", "")).build();

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.append(&header);
    outer.append(&content);

    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(420)
        .content(&outer)
        .build();
    dialog.present();
}
