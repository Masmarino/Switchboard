import AppKit
import SwiftUI

struct ContentView: View {
    @Bindable var state: AppState

    var body: some View {
        NavigationSplitView {
            List(state.apps, selection: $state.selectedID) { app in
                AppRowView(
                    app: app,
                    onStart: { state.startApp(app.id) },
                    onStop: { state.stopApp(app.id) },
                    onRemove: { state.removeApp(app.id) },
                    onEdit: { state.editingApp = app }
                )
                .tag(app.id)
            }
            .listStyle(.sidebar)
            .navigationTitle("Switchboard")
        } detail: {
            VStack(spacing: 0) {
                HStack {
                    Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                    TextField("Filtrer les logs…", text: $state.logFilter)
                        .textFieldStyle(.plain)
                    if !state.logFilter.isEmpty {
                        Button(action: { state.logFilter = "" }) {
                            Image(systemName: "xmark.circle.fill")
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(.secondary)
                    }
                }
                .padding(8)
                .background(.bar)

                Divider()

                LogPanelView(hasApp: state.selected != nil, lines: state.filteredLogs)
                    .padding(10)
            }
            .background(.background)
            .navigationTitle(state.selected?.name ?? "Switchboard")
            .navigationSubtitle(state.selected?.statusLabel ?? "")
        }
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Button(action: { state.addSheetPresented = true }) {
                    Image(systemName: "plus")
                }
                .help("Ajouter une app")

                Button(action: { state.startAll() }) {
                    Image(systemName: "play.fill")
                }
                .help("Tout démarrer")

                Button(action: { state.stopAll() }) {
                    Image(systemName: "stop.fill")
                }
                .help("Tout arrêter")

                Button(action: { state.clearLogs() }) {
                    Image(systemName: "trash.slash")
                }
                .help("Effacer les logs")
                .disabled(state.selected == nil)

                Button(action: exportLogs) {
                    Image(systemName: "square.and.arrow.down")
                }
                .help("Exporter les logs…")
                .disabled(state.selected == nil)
            }
        }
        .sheet(isPresented: $state.addSheetPresented) {
            AddAppSheet(existing: nil, validate: { state.validate($0, editingID: nil) }) { draft in state.addApp(draft) }
        }
        .sheet(item: $state.editingApp) { app in
            AddAppSheet(existing: app, validate: { state.validate($0, editingID: app.id) }) { draft in
                state.updateApp(id: app.id, draft: draft)
            }
        }
        .sheet(isPresented: $state.exportSheetPresented) {
            ConfigExportSheet(apps: state.apps) { ids, includeEnvVars in
                state.exportConfig(ids: ids, includeEnvVars: includeEnvVars)
            }
        }
        .task { state.start() }
    }

    private func exportLogs() {
        guard let app = state.selected else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "\(app.name).log"
        if panel.runModal() == .OK, let url = panel.url {
            state.exportLogs(id: app.id, toPath: url.path)
        }
    }
}
