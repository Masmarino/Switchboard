# Switchboard

Un lanceur de process de dev léger et natif. Démarre, arrête et supervise tes serveurs locaux — Rust, Node, .NET, Java, Python, Go, ou n'importe quelle commande — depuis une app native macOS, Windows ou Linux, sans jamais quitter ton clavier pour un terminal de plus.

## Télécharger

Les builds pour macOS, Windows et Linux sont disponibles sur la page [Releases](https://github.com/masmarino/switchboard/releases).

- **macOS** : `Switchboard-x.y.z-macos.dmg` — glisse l'icône dans Applications.
- **Windows** : `Switchboard-x.y.z-setup.exe` — installeur (recommandé), demande les droits admin (un seul prompt UAC) car il installe aussi le [Visual C++ Redistributable](https://aka.ms/vs/17/release/vc_redist.x64.exe) requis par WinUI3 si besoin ; `switchboard-x.y.z-windows.zip` reste disponible en version portable (dézipper et lancer), mais nécessite alors d'avoir le Visual C++ Redistributable x64 déjà installé (sinon l'app crashe au lancement, module `Microsoft.UI.Xaml.dll`).
- **Linux** : `switchboard-x.y.z-linux.tar.gz` (nécessite `libgtk-4`/`libadwaita-1`) — extraire, puis lancer `./install.sh` pour une intégration propre au menu d'applications (nom et icône corrects, plutôt que l'identifiant brut `com.skolln.switchboard` avec une icône générique). Sans `install.sh`, le binaire `bin/switchboard` reste utilisable directement.

## Fonctionnalités

- **Langages/outils supportés** : Cargo (Rust), Npm, Dotnet, Maven (Java), Python, Go, et Raw (commande shell arbitraire pour tout le reste)
- Démarrer/arrêter chaque app individuellement, ou **tout démarrer/arrêter** d'un coup
- Logs en direct avec **recherche/filtre** par mot-clé
- **Effacer les logs** d'une app, ou les **exporter** vers un fichier
- **Ouvrir dans le navigateur** (bouton dédié si l'app a une URL configurée)
- **Modifier une app** existante (nom, dossier, type, commande, URL, variables d'env, auto-restart)
- **Variables d'environnement** par app, injectées dans le process lancé
- **Auto-restart** : relance automatique en cas de crash
- **Notifications natives** quand un process crashe (toast macOS/Windows, notification GNOME sur Linux)
- **Vérification de santé** : ping périodique de l'URL configurée pour distinguer "le process tourne" de "l'app répond vraiment" (✓ healthy / ✗ ne répond pas)
- **Usage CPU/mémoire** par app, affiché en direct
- **Ordre de démarrage** : "Tout démarrer" lance les apps par paliers, pour laisser une dépendance (ex: une API) démarrer avant ses dépendants
- **Icône menu bar / tray** pour démarrer/arrêter sans rouvrir la fenêtre principale
- **CLI pour scripts et agents IA** (`switchboard-cli`) : créer, modifier, supprimer et piloter les apps, sortie JSON stable — voir plus bas
- Aucun compte, aucun cloud — la config reste en local

## Types d'app et commandes par défaut

| Type | Commande lancée | Champ "Commande" |
|------|------------------|-------------------|
| Cargo | `cargo build` puis `cargo run` | ignoré |
| Npm | `npm <commande>` | obligatoire (ex: `start`) |
| Dotnet | `dotnet run [commande]` | optionnel (ex: `--project Api.csproj`) |
| Maven | `mvn <commande>` | optionnel, défaut `spring-boot:run` |
| Python | `python3 <commande>` | optionnel, défaut `main.py` |
| Go | `go run <commande>` | optionnel, défaut `.` |
| Raw | `sh -c "<commande>"` | obligatoire |

## CLI (`switchboard-cli`)

Un CLI permet de gérer les apps de lancement sans ouvrir la fenêtre, pensé pour qu'une IA comme Claude Code puisse les créer, les modifier et les piloter : `--json` sur toutes les commandes, codes de sortie documentés, aucune question interactive, et `switchboard-cli schema` qui décrit les champs attendus.

```sh
make cli                                   # installe switchboard-cli dans ~/.cargo/bin
switchboard-cli add --name "Alume API" --dir . --kind cargo --url http://localhost:3000/health
switchboard-cli start "alume api" --wait healthy
switchboard-cli logs "alume api" --tail 50
switchboard-cli stop --all
```

- Le CLI modifie le même `apps.json` que les apps natives (verrou + écriture atomique) ; une fenêtre Switchboard ouverte recharge le fichier en moins d'une seconde.
- Les commandes de process passent par le « superviseur » : la fenêtre Switchboard si elle est ouverte, sinon un daemon lancé en arrière-plan à la demande (`switchboard-cli daemon stop` pour l'arrêter). À l'ouverture de la fenêtre, le daemon lui passe la main et elle relance ses apps.

Guide complet pour les agents (contrat JSON, codes d'erreur, recettes) : [docs/cli.md](docs/cli.md).

## Pourquoi pas un seul framework cross-platform ?

Aucun moteur de rendu immédiat (egui, etc.) ne peut afficher le vrai flou/vibrancy du compositeur (Liquid Glass macOS, Mica Windows) ni de vrais widgets système. Switchboard est donc écrit dans la techno native de chaque OS, et ne partage que la logique métier via un cœur Rust commun :

```
core/      switchboard-core   — logique partagée (config, process, logs, santé) — pure Rust, aucune dépendance UI
ffi/       switchboard-ffi    — shim C ABI (JSON) sur core, pour macOS/Windows
cli/       switchboard-cli    — CLI (scripts, agents IA), dépend de core directement
linux/     switchboard-linux  — GTK4 + libadwaita, dépend de core directement (Rust)
macos/     SwiftUI            — NavigationSplitView + Liquid Glass, via le shim FFI
windows/   WinUI3 / C#        — Mica, via le shim FFI (P/Invoke)
```

Chaque frontend suit nativement le mode clair/sombre de son OS, et utilise les vrais widgets/dialogues système (sélecteur de fichiers, notifications, "À propos"...).

**État de test** : macOS et Linux sont buildés et testés réellement (macOS nativement, Linux via le backend macOS de GTK4 — le rendu visuel diffère d'un vrai bureau GNOME mais le code et les widgets Adwaita sont réels). **Windows/WinUI3 est écrit par analogie mais n'a jamais été compilé ni lancé** faute de machine Windows — contributions bienvenues pour le vérifier/corriger.

**Icône de tray Linux non implémentée** : GNOME n'a pas de support natif pour les icônes de tray (StatusNotifierItem) sans extension. Une intégration via `ksni` (le protocole D-Bus StatusNotifierItem) reste à faire et à tester sur une vraie machine Linux.

## Développer

```sh
make macos      # build + installe dans /Applications
make linux      # build + lance (necessite gtk4-dev / libadwaita-dev)
make windows    # affiche l'instruction (lancer scripts/build-windows.ps1 depuis PowerShell)
make cli        # installe switchboard-cli (cargo install)
make test       # tests Rust (core, ffi, linux, cli)
```

### Le pont FFI (macOS / Windows)

`ffi/` expose une poignée de fonctions C ABI (`switchboard_engine_new`, `_list_apps_json`, `_add_app_json`, `_update_app_json`, `_remove_app`, `_start_app`, `_stop_app`, `_start_all`, `_stop_all`, `_clear_logs`, `_export_logs`, `_free`) consommées :
- en Swift via un module système (`macos/Sources/CSwitchboardFFI`) généré par `cbindgen` (header dans `ffi/include/`, copié dans le module Swift),
- en C# via `[DllImport]` / P/Invoke (`windows/Engine/DevtoolEngine.cs`).

Toutes les structures riches (liste d'apps, statuts, logs) passent en JSON plutôt qu'en marshaling manuel de structs. Linux n'a pas besoin de ce pont : `switchboard-linux` dépend de `switchboard-core` directement en Rust.

Pour régénérer le header après une modification de `ffi/src/lib.rs` :

```sh
cd ffi && cbindgen --config cbindgen.toml --crate switchboard-ffi --output include/switchboard_ffi.h
cp include/switchboard_ffi.h ../macos/Sources/CSwitchboardFFI/
```

## Licence

[GPLv3](LICENSE).

## Crédits

Développé par [SkollN](https://www.skolln.com). Découvre aussi [Alume](https://alume.skolln.com), notre agrégateur de contenus (articles + podcasts) avec IA intégrée pour iOS, macOS et Android.
