/// Probleme sur un champ, renvoye par la validation du moteur (memes regles que le CLI).
struct FieldIssue: Decodable, Hashable {
    let field: String
    let code: String
    let message: String

    /// Le moteur repond en anglais (messages partages avec le CLI) ; on traduit les codes connus.
    var localizedMessage: String {
        switch (field, code) {
        case ("name", "required"): "Le nom est obligatoire."
        case ("name", "duplicate"): "Une autre app porte déjà ce nom."
        case ("working_dir", "required"): "Le dossier est obligatoire."
        case ("working_dir", "not_absolute"): "Le dossier doit être un chemin absolu."
        case ("working_dir", "missing"): "Ce dossier n'existe pas (encore) : le démarrage échouera."
        case ("command", "required"): "La commande est obligatoire pour ce type."
        case ("command", "unbalanced_quotes"): "Un guillemet n'est pas refermé dans la commande."
        case ("url", "invalid"): "L'URL doit commencer par http:// ou https:// et ne pas contenir d'espace."
        case ("env_vars", "invalid_key"): "Nom de variable invalide (pas d'espace ni de « = »)."
        case ("env_vars", "duplicate_key"): "Une variable d'environnement est définie deux fois."
        case ("env_vars", "invalid_value"): "Une valeur de variable contient un caractère nul."
        case ("start_order", "out_of_range"): "L'ordre de démarrage doit être entre 0 et 99."
        default: message
        }
    }
}
