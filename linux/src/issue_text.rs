use switchboard_core::FieldIssue;

/// Le moteur repond en anglais (messages partages avec le CLI) ; on traduit les codes connus.
pub fn issue_text(issue: &FieldIssue) -> String {
    let text = match (issue.field, issue.code) {
        ("name", "required") => "Le nom est obligatoire.",
        ("name", "duplicate") => "Une autre app porte déjà ce nom.",
        ("working_dir", "required") => "Le dossier est obligatoire.",
        ("working_dir", "not_absolute") => "Le dossier doit être un chemin absolu.",
        ("working_dir", "missing") => "Ce dossier n'existe pas (encore) : le démarrage échouera.",
        ("command", "required") => "La commande est obligatoire pour ce type.",
        ("command", "unbalanced_quotes") => "Un guillemet n'est pas refermé dans la commande.",
        ("url", "invalid") => "L'URL doit commencer par http:// ou https:// et ne pas contenir d'espace.",
        ("env_vars", "invalid_key") => "Nom de variable invalide (pas d'espace ni de « = »).",
        ("env_vars", "duplicate_key") => "Une variable d'environnement est définie deux fois.",
        ("env_vars", "invalid_value") => "Une valeur de variable contient un caractère nul.",
        ("start_order", "out_of_range") => "L'ordre de démarrage doit être entre 0 et 99.",
        _ => return issue.message.clone(),
    };
    text.to_string()
}
