/// Role du champ "Commande" selon le type d'app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRule {
    /// Le type lance une commande fixe (Cargo : `cargo build` puis `cargo run`).
    Ignored,
    /// Sans commande, le process n'a rien a lancer (Npm, Raw).
    Required,
    /// Arguments optionnels ; `default` est utilise quand le champ est vide.
    Optional { default: Option<&'static str> },
}

impl CommandRule {
    pub fn as_str(self) -> &'static str {
        match self {
            CommandRule::Ignored => "ignored",
            CommandRule::Required => "required",
            CommandRule::Optional { .. } => "optional",
        }
    }
}
