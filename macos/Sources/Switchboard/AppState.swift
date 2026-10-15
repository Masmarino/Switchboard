import AppKit
import Foundation
import Observation
import UniformTypeIdentifiers
import UserNotifications

@MainActor
@Observable
final class AppState {
    private let engine = DevtoolEngine()
    private var pollTask: Task<Void, Never>?
    /// Sans activite declaree, App Nap espace le poll quand la fenetre est cachee : les apps
    /// supervisees et les requetes du CLI attendraient alors plusieurs secondes.
    @ObservationIgnored private var appNapActivity: NSObjectProtocol?
    private var lastStatus: [String: String] = [:]
    private var lastSeenRevision: UInt64 = 0
    private var sinceSeq: UInt64 = 0
    var selectedLogs: [LogLine] = []
    /// Mirrors the Rust engine's MAX_LOG_LINES cap so the array can't grow unbounded.
    private static let maxDisplayedLogLines = 5000

    var apps: [AppEntry] = []
    var selectedID: String? {
        didSet {
            guard selectedID != oldValue else { return }
            sinceSeq = 0
            selectedLogs = []
            refresh(force: true)
        }
    }
    var addSheetPresented = false
    var exportSheetPresented = false
    var editingApp: AppEntry?
    var logFilter = ""

    var selected: AppEntry? {
        apps.first { $0.id == selectedID } ?? apps.first
    }

    var filteredLogs: [LogLine] {
        guard selected != nil else { return [] }
        guard !logFilter.isEmpty else { return selectedLogs }
        return selectedLogs.filter { $0.text.localizedCaseInsensitiveContains(logFilter) }
    }

    func start() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
        appNapActivity = ProcessInfo.processInfo.beginActivity(
            options: .userInitiatedAllowingIdleSystemSleep,
            reason: "Supervising dev processes and answering switchboard-cli"
        )
        refresh(force: true)
        pollTask = Task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(200))
                let rev = engine.revision()
                if rev != lastSeenRevision {
                    lastSeenRevision = rev
                    refresh(force: false)
                }
            }
        }
    }

    func stop() {
        pollTask?.cancel()
        if let appNapActivity {
            ProcessInfo.processInfo.endActivity(appNapActivity)
        }
    }

    func refresh(force: Bool) {
        let fetched = engine.listApps(selectedID: selectedID, sinceSeq: sinceSeq)
        if let id = selectedID, let selectedEntry = fetched.first(where: { $0.id == id }) {
            if selectedEntry.logsReplace {
                selectedLogs = makeLogLines(selectedEntry.logs, baseSeq: selectedEntry.logsBaseSeq)
            } else if !selectedEntry.logs.isEmpty {
                selectedLogs.append(contentsOf: makeLogLines(selectedEntry.logs, baseSeq: selectedEntry.logsBaseSeq))
                if selectedLogs.count > Self.maxDisplayedLogLines {
                    selectedLogs.removeFirst(selectedLogs.count - Self.maxDisplayedLogLines)
                }
            }
            sinceSeq = selectedEntry.logsBaseSeq + UInt64(selectedEntry.logs.count)
        }
        if force || fetched != apps {
            apps = fetched
        }
        notifyNewFailures()
        if selectedID == nil {
            selectedID = apps.first?.id
        }
    }

    private func notifyNewFailures() {
        for app in apps {
            let previous = lastStatus[app.id]
            lastStatus[app.id] = app.statusLabel
            if app.statusLabel == "failed", previous != "failed" {
                let content = UNMutableNotificationContent()
                content.title = "\(app.name) a crashé"
                content.body = app.error ?? "Le process s'est arrêté de manière inattendue."
                content.sound = .default
                let request = UNNotificationRequest(identifier: "crash-\(app.id)", content: content, trigger: nil)
                UNUserNotificationCenter.current().add(request)
            }
        }
    }

    func startApp(_ id: String) {
        engine.startApp(id: id)
        selectedID = id
        refresh(force: true)
    }

    func stopApp(_ id: String) {
        engine.stopApp(id: id)
        refresh(force: true)
    }

    func removeApp(_ id: String) {
        engine.removeApp(id: id)
        if selectedID == id {
            selectedID = nil
        }
        refresh(force: true)
    }

    func startAll() {
        engine.startAll()
        refresh(force: true)
    }

    func stopAll() {
        engine.stopAll()
        refresh(force: true)
    }

    /// deinit doesn't run on quit, so call this explicitly from the app delegate: waits for the
    /// processes to exit (no orphans holding ports) and removes the CLI control files.
    func stopAllForShutdown() {
        engine.shutdown()
    }

    func clearLogs() {
        guard let id = selectedID else { return }
        engine.clearLogs(id: id)
        selectedLogs = []
        sinceSeq = 0
        refresh(force: true)
    }

    /// logsBaseSeq is already a stable, monotonic id base — no separate counter needed.
    private func makeLogLines(_ texts: [String], baseSeq: UInt64) -> [LogLine] {
        texts.enumerated().map { offset, text in
            LogLine(id: Int(baseSeq) + offset, text: text)
        }
    }

    @discardableResult
    func exportLogs(id: String, toPath path: String) -> Bool {
        engine.exportLogs(id: id, path: path)
    }

    func addApp(_ draft: AppDraftPayload) {
        engine.addApp(draft)
        refresh(force: true)
    }

    func validate(_ draft: AppDraftPayload, editingID: String?) -> DraftValidation {
        engine.validate(draft: draft, editingID: editingID) ?? .empty
    }

    func updateApp(id: String, draft: AppDraftPayload) {
        engine.updateApp(id: id, draft: draft)
        refresh(force: true)
    }

    func exportConfig(ids: [String], includeEnvVars: Bool) -> String? {
        engine.exportConfig(ids: ids, includeEnvVars: includeEnvVars)
    }

    /// Panneau natif -> apercu -> confirmation -> application, sans sheet SwiftUI dediee.
    func importConfig() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.json]
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url, let json = try? String(contentsOf: url, encoding: .utf8) else {
            return
        }
        guard let preview = engine.previewImportConfig(json: json) else {
            showAlert(title: "Fichier invalide", message: "Ce fichier ne contient pas une configuration Switchboard valide.")
            return
        }
        if preview.toAdd.isEmpty && preview.toReplace.isEmpty {
            showAlert(title: "Rien à importer", message: "Ce fichier ne contient aucune app à ajouter ou remplacer.")
            return
        }
        var lines: [String] = []
        if !preview.toAdd.isEmpty {
            lines.append("\(preview.toAdd.count) app(s) seront ajoutées : \(preview.toAdd.joined(separator: ", "))")
        }
        if !preview.toReplace.isEmpty {
            lines.append("\(preview.toReplace.count) app(s) seront remplacées : \(preview.toReplace.joined(separator: ", "))")
        }
        let alert = NSAlert()
        alert.messageText = "Importer cette configuration ?"
        alert.informativeText = lines.joined(separator: "\n")
        alert.addButton(withTitle: "Importer")
        alert.addButton(withTitle: "Annuler")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        _ = engine.applyImportConfig(json: json)
        refresh(force: true)
    }

    private func showAlert(title: String, message: String) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }
}
