import CSwitchboardFFI
import Foundation
import OSLog

private let logger = Logger(subsystem: "com.skolln.switchboard", category: "Engine")

private let jsonDecoder: JSONDecoder = {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    return decoder
}()

final class DevtoolEngine {
    private let handle: OpaquePointer

    init() {
        handle = switchboard_engine_new()
    }

    deinit {
        switchboard_engine_free(handle)
    }

    func listApps(selectedID: String?, sinceSeq: UInt64) -> [AppEntry] {
        let raw: UnsafeMutablePointer<CChar>?
        if let selectedID {
            raw = selectedID.withCString { switchboard_engine_list_apps_json(handle, $0, sinceSeq) }
        } else {
            raw = switchboard_engine_list_apps_json(handle, nil, sinceSeq)
        }
        guard let raw else { return [] }
        defer { switchboard_string_free(raw) }
        let data = Data(String(cString: raw).utf8)
        do {
            return try jsonDecoder.decode([AppEntry].self, from: data)
        } catch {
            logger.error("Failed to decode app list: \(error.localizedDescription)")
            return []
        }
    }

    func revision() -> UInt64 {
        switchboard_engine_revision(handle)
    }

    func addApp(_ draft: AppDraftPayload) {
        withDraftJSON(draft) { json in
            withCStrings(json) { json in switchboard_engine_add_app_json(handle, json) }
        }
    }

    func updateApp(id: String, draft: AppDraftPayload) {
        withDraftJSON(draft) { json in
            withCStrings(id, json) { id, json in switchboard_engine_update_app_json(handle, id, json) }
        }
    }

    /// Memes regles que le CLI ; `nil` si le moteur n'a pas pu lire le draft.
    func validate(draft: AppDraftPayload, editingID: String?) -> DraftValidation? {
        var result: DraftValidation?
        withDraftJSON(draft) { json in
            json.withCString { jsonPtr in
                let raw = editingID.map { id in
                    id.withCString { switchboard_engine_validate_draft_json(handle, $0, jsonPtr) }
                } ?? switchboard_engine_validate_draft_json(handle, nil, jsonPtr)
                guard let raw else { return }
                defer { switchboard_string_free(raw) }
                result = try? jsonDecoder.decode(DraftValidation.self, from: Data(String(cString: raw).utf8))
            }
        }
        return result
    }

    /// Arrete les apps et attend leur fin, puis ferme le canal du CLI.
    func shutdown() {
        switchboard_engine_shutdown(handle)
    }

    func removeApp(id: String) {
        withCStrings(id) { id in switchboard_engine_remove_app(handle, id) }
    }

    func startApp(id: String) {
        withCStrings(id) { id in switchboard_engine_start_app(handle, id) }
    }

    func stopApp(id: String) {
        withCStrings(id) { id in switchboard_engine_stop_app(handle, id) }
    }

    func startAll() {
        switchboard_engine_start_all(handle)
    }

    func stopAll() {
        switchboard_engine_stop_all(handle)
    }

    func exportConfig(ids: [String], includeEnvVars: Bool) -> String? {
        guard let idsData = try? JSONEncoder().encode(ids), let idsJSON = String(data: idsData, encoding: .utf8) else {
            return nil
        }
        var result: String?
        withCStrings(idsJSON) { idsPtr in
            if let raw = switchboard_engine_export_config_json(handle, idsPtr, includeEnvVars) {
                defer { switchboard_string_free(raw) }
                result = String(cString: raw)
            }
        }
        return result
    }

    func previewImportConfig(json: String) -> ImportSummary? {
        var result: ImportSummary?
        withCStrings(json) { jsonPtr in result = decodeSummary(switchboard_engine_preview_import_json(handle, jsonPtr)) }
        return result
    }

    func applyImportConfig(json: String) -> ImportSummary? {
        var result: ImportSummary?
        withCStrings(json) { jsonPtr in result = decodeSummary(switchboard_engine_apply_import_json(handle, jsonPtr)) }
        return result
    }

    private func decodeSummary(_ raw: UnsafeMutablePointer<CChar>?) -> ImportSummary? {
        guard let raw else { return nil }
        defer { switchboard_string_free(raw) }
        let data = Data(String(cString: raw).utf8)
        return try? jsonDecoder.decode(ImportSummary.self, from: data)
    }

    func clearLogs(id: String) {
        withCStrings(id) { id in switchboard_engine_clear_logs(handle, id) }
    }

    @discardableResult
    func exportLogs(id: String, path: String) -> Bool {
        var result = false
        withCStrings(id, path) { id, path in result = switchboard_engine_export_logs(handle, id, path) }
        return result
    }

    private func withDraftJSON(_ draft: AppDraftPayload, _ body: (String) -> Void) {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        guard let data = try? encoder.encode(draft), let json = String(data: data, encoding: .utf8) else {
            logger.error("Failed to encode app draft")
            return
        }
        body(json)
    }
}

private func withCStrings(_ a: String, _ body: (UnsafePointer<CChar>) -> Void) {
    a.withCString { body($0) }
}

private func withCStrings(
    _ a: String, _ b: String,
    _ body: (UnsafePointer<CChar>, UnsafePointer<CChar>) -> Void
) {
    a.withCString { a in
        b.withCString { b in
            body(a, b)
        }
    }
}
