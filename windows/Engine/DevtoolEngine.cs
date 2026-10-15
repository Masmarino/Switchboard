using System.Runtime.InteropServices;
using System.Text.Json;
using Switchboard.Models;

namespace Switchboard.Engine;

/// <summary>Wrapper P/Invoke sur switchboard_ffi.dll ; tout passe en JSON, pas de marshaling de structs.</summary>
public sealed class DevtoolEngine : IDisposable
{
    private const string Dll = "switchboard_ffi";

    [DllImport(Dll)] private static extern IntPtr switchboard_engine_new();
    [DllImport(Dll)] private static extern void switchboard_engine_free(IntPtr engine);
    [DllImport(Dll)] private static extern IntPtr switchboard_engine_list_apps_json(IntPtr engine, string? selectedId, ulong sinceSeq);
    [DllImport(Dll)] private static extern ulong switchboard_engine_revision(IntPtr engine);
    [DllImport(Dll)] private static extern void switchboard_engine_add_app_json(IntPtr engine, string draftJson);
    [DllImport(Dll)] private static extern void switchboard_engine_update_app_json(IntPtr engine, string id, string draftJson);
    [DllImport(Dll)] private static extern IntPtr switchboard_engine_validate_draft_json(IntPtr engine, string? editingId, string draftJson);
    [DllImport(Dll)] private static extern void switchboard_engine_shutdown(IntPtr engine);
    [DllImport(Dll)] private static extern void switchboard_engine_remove_app(IntPtr engine, string id);
    [DllImport(Dll)] private static extern void switchboard_engine_start_app(IntPtr engine, string id);
    [DllImport(Dll)] private static extern void switchboard_engine_stop_app(IntPtr engine, string id);
    [DllImport(Dll)] private static extern void switchboard_engine_start_all(IntPtr engine);
    [DllImport(Dll)] private static extern void switchboard_engine_stop_all(IntPtr engine);
    [DllImport(Dll)] private static extern void switchboard_engine_clear_logs(IntPtr engine, string id);
    [DllImport(Dll)] private static extern bool switchboard_engine_export_logs(IntPtr engine, string id, string path);
    [DllImport(Dll)] private static extern IntPtr switchboard_engine_export_config_json(IntPtr engine, string idsJson, bool includeEnvVars);
    [DllImport(Dll)] private static extern IntPtr switchboard_engine_preview_import_json(IntPtr engine, string configJson);
    [DllImport(Dll)] private static extern IntPtr switchboard_engine_apply_import_json(IntPtr engine, string configJson);
    [DllImport(Dll)] private static extern void switchboard_string_free(IntPtr s);

    private readonly IntPtr _handle;

    public DevtoolEngine()
    {
        _handle = switchboard_engine_new();
    }

    public List<AppEntry> ListApps(string? selectedId, ulong sinceSeq)
    {
        var json = ReadAndFreeString(switchboard_engine_list_apps_json(_handle, selectedId, sinceSeq)) ?? "[]";
        return JsonSerializer.Deserialize<List<AppEntry>>(json) ?? [];
    }

    public ulong Revision() => switchboard_engine_revision(_handle);

    public void AddApp(AppDraftPayload draft) =>
        switchboard_engine_add_app_json(_handle, JsonSerializer.Serialize(draft));

    public void UpdateApp(string id, AppDraftPayload draft) =>
        switchboard_engine_update_app_json(_handle, id, JsonSerializer.Serialize(draft));

    /// <summary>Memes regles que le CLI ; null si le moteur n'a pas pu lire le draft.</summary>
    public DraftValidation? Validate(AppDraftPayload draft, string? editingId)
    {
        var json = ReadAndFreeString(switchboard_engine_validate_draft_json(_handle, editingId, JsonSerializer.Serialize(draft)));
        return json is null ? null : JsonSerializer.Deserialize<DraftValidation>(json);
    }

    /// <summary>Arrete les apps et attend leur fin, puis ferme le canal du CLI.</summary>
    public void Shutdown() => switchboard_engine_shutdown(_handle);

    public void RemoveApp(string id) => switchboard_engine_remove_app(_handle, id);

    public void StartApp(string id) => switchboard_engine_start_app(_handle, id);

    public void StopApp(string id) => switchboard_engine_stop_app(_handle, id);

    public void StartAll() => switchboard_engine_start_all(_handle);

    public void StopAll() => switchboard_engine_stop_all(_handle);

    public void ClearLogs(string id) => switchboard_engine_clear_logs(_handle, id);

    public bool ExportLogs(string id, string path) => switchboard_engine_export_logs(_handle, id, path);

    public string ExportConfig(List<string> ids, bool includeEnvVars) =>
        ReadAndFreeString(switchboard_engine_export_config_json(_handle, JsonSerializer.Serialize(ids), includeEnvVars)) ?? "{}";

    public ImportSummary? PreviewImportConfig(string configJson) => DecodeSummary(switchboard_engine_preview_import_json(_handle, configJson));

    public ImportSummary? ApplyImportConfig(string configJson) => DecodeSummary(switchboard_engine_apply_import_json(_handle, configJson));

    private static ImportSummary? DecodeSummary(IntPtr raw)
    {
        var json = ReadAndFreeString(raw);
        return json is null ? null : JsonSerializer.Deserialize<ImportSummary>(json);
    }

    private static string? ReadAndFreeString(IntPtr raw)
    {
        if (raw == IntPtr.Zero) return null;
        try
        {
            return Marshal.PtrToStringUTF8(raw);
        }
        finally
        {
            switchboard_string_free(raw);
        }
    }

    public void Dispose() => switchboard_engine_free(_handle);
}
