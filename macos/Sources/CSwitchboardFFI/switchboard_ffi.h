// Genere par cbindgen — ne pas editer a la main.

#ifndef SWITCHBOARD_FFI_H
#define SWITCHBOARD_FFI_H

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

typedef struct AppKind AppKind;

/**
 * Facade sur la config, les process en cours et leurs logs — pas de dependance UI.
 */
typedef struct Engine Engine;



struct Engine *switchboard_engine_new(void);

/**
 * # Safety
 * `engine` doit provenir de [`switchboard_engine_new`] et ne doit plus etre utilise apres cet appel.
 */
void switchboard_engine_free(struct Engine *engine);

/**
 * Liste des apps en JSON — seule `selected_id` recoit ses logs (lignes posterieures
 * a `since_seq`, sauf remplacement complet via `logs_replace`). A liberer avec
 * [`switchboard_string_free`].
 *
 * # Safety
 * `engine` doit etre un pointeur valide retourne par [`switchboard_engine_new`].
 * `selected_id`, si non-NULL, doit etre une chaine C valide.
 */
char *switchboard_engine_list_apps_json(struct Engine *engine,
                                        const char *selected_id,
                                        uint64_t since_seq);

/**
 * Renvoie la revision courante du moteur (compteur qui n'avance que si quelque
 * chose a reellement change). Bon marche : pas de JSON, pas d'allocation.
 *
 * # Safety
 * `engine` doit etre un pointeur valide retourne par [`switchboard_engine_new`].
 */
uint64_t switchboard_engine_revision(struct Engine *engine);

/**
 * `draft_json` doit correspondre a [`FfiAppDraft`] (cf. doc du module).
 *
 * # Safety
 * `engine` doit etre un pointeur valide. `draft_json` doit etre une chaine C valide.
 */
void switchboard_engine_add_app_json(struct Engine *engine, const char *draft_json);

/**
 * # Safety
 * `engine` doit etre un pointeur valide. `id` et `draft_json` doivent etre des
 * chaines C valides ; `draft_json` doit correspondre a [`FfiAppDraft`].
 */
void switchboard_engine_update_app_json(struct Engine *engine,
                                        const char *id,
                                        const char *draft_json);

/**
 * Valide un draft avec les memes regles que le CLI. Retourne
 * `{"errors":[{field,code,message}],"warnings":[...]}` (listes vides si tout va bien),
 * a liberer avec [`switchboard_string_free`]. `editing_id` (ou NULL) exclut l'app modifiee
 * du controle d'unicite du nom.
 *
 * # Safety
 * `engine` doit etre un pointeur valide. `draft_json` doit etre une chaine C valide ;
 * `editing_id`, si non-NULL, aussi.
 */
char *switchboard_engine_validate_draft_json(struct Engine *engine,
                                             const char *editing_id,
                                             const char *draft_json);

/**
 * Arrete toutes les apps et attend leur fin (SIGKILL apres quelques secondes), puis ferme
 * le canal du CLI. A appeler a la fermeture de l'app, avant [`switchboard_engine_free`].
 *
 * # Safety
 * `engine` doit etre un pointeur valide.
 */
void switchboard_engine_shutdown(struct Engine *engine);

/**
 * # Safety
 * `engine` doit etre un pointeur valide. `id` doit etre une chaine C valide (UUID).
 */
void switchboard_engine_remove_app(struct Engine *engine, const char *id);

/**
 * # Safety
 * `engine` doit etre un pointeur valide. `id` doit etre une chaine C valide (UUID).
 */
void switchboard_engine_start_app(struct Engine *engine, const char *id);

/**
 * # Safety
 * `engine` doit etre un pointeur valide. `id` doit etre une chaine C valide (UUID).
 */
void switchboard_engine_stop_app(struct Engine *engine, const char *id);

/**
 * # Safety
 * `engine` doit etre un pointeur valide. `id` doit etre une chaine C valide (UUID).
 */
void switchboard_engine_clear_logs(struct Engine *engine, const char *id);

/**
 * Exporte les logs courants d'une app vers un fichier. Retourne `true` en cas de succes.
 *
 * # Safety
 * `engine` doit etre un pointeur valide. `id` et `path` doivent etre des chaines C valides.
 */
bool switchboard_engine_export_logs(struct Engine *engine, const char *id, const char *path);

/**
 * # Safety
 * `engine` doit etre un pointeur valide.
 */
void switchboard_engine_start_all(struct Engine *engine);

/**
 * # Safety
 * `engine` doit etre un pointeur valide.
 */
void switchboard_engine_stop_all(struct Engine *engine);

/**
 * Sous-ensemble d'apps en JSON (`ids_json` = tableau de chaines UUID), a liberer
 * avec [`switchboard_string_free`].
 *
 * # Safety
 * `engine` doit etre un pointeur valide. `ids_json` doit etre une chaine C valide.
 */
char *switchboard_engine_export_config_json(struct Engine *engine,
                                            const char *ids_json,
                                            bool include_env_vars);

/**
 * Calcule l'apercu de fusion (compte + noms) sans rien modifier. Retourne NULL si
 * `config_json` n'est pas un fichier de config Switchboard valide.
 *
 * # Safety
 * `engine` doit etre un pointeur valide. `config_json` doit etre une chaine C valide.
 */
char *switchboard_engine_preview_import_json(struct Engine *engine, const char *config_json);

/**
 * Applique reellement la fusion et persiste. Retourne NULL si `config_json` n'est
 * pas un fichier de config Switchboard valide (rien n'est modifie dans ce cas).
 *
 * # Safety
 * `engine` doit etre un pointeur valide. `config_json` doit etre une chaine C valide.
 */
char *switchboard_engine_apply_import_json(struct Engine *engine, const char *config_json);

/**
 * Libere une chaine retournee par une fonction de ce module.
 *
 * # Safety
 * `s` doit provenir d'une fonction de ce module (ou etre NULL), et ne doit plus
 * etre utilise apres cet appel.
 */
void switchboard_string_free(char *s);

#endif  /* SWITCHBOARD_FFI_H */
