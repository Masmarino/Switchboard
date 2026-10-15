using System.Text.Json.Serialization;

namespace Switchboard.Models;

/// <summary>Probleme sur un champ, renvoye par la validation du moteur (memes regles que le CLI).</summary>
public class FieldIssue
{
    [JsonPropertyName("field")]
    public string Field { get; set; } = "";

    [JsonPropertyName("code")]
    public string Code { get; set; } = "";

    [JsonPropertyName("message")]
    public string Message { get; set; } = "";

    /// <summary>Le moteur repond en anglais (messages partages avec le CLI) ; on traduit les codes connus.</summary>
    public string LocalizedMessage => (Field, Code) switch
    {
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
        _ => Message,
    };
}
