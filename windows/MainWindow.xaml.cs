using System.IO;
using System.Linq;
using Microsoft.UI;
using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Microsoft.Windows.AppNotifications;
using Windows.UI;
using Microsoft.UI.Text;
using Switchboard.Engine;
using Switchboard.Models;
using WinForms = System.Windows.Forms;

namespace Switchboard;

public sealed partial class MainWindow : Window
{
    private static readonly (AppKind Kind, string Label)[] KindOptions =
    [
        (AppKind.Cargo, "Cargo"),
        (AppKind.Npm, "Npm"),
        (AppKind.Dotnet, "Dotnet"),
        (AppKind.Maven, "Maven"),
        (AppKind.Python, "Python"),
        (AppKind.Go, "Go"),
        (AppKind.Raw, "Raw"),
    ];

    private readonly DevtoolEngine _engine = new();
    private readonly DispatcherTimer _timer = new();
    private WinForms.NotifyIcon? _trayIcon;
    private string? _selectedId;
    private List<AppEntry> _apps = [];
    private readonly Dictionary<string, string> _lastStatus = [];
    private ulong _lastSeenRevision;
    private ulong _sinceSeq;
    private readonly List<string> _selectedLogs = [];
    /// Mirrors the Rust engine's MAX_LOG_LINES cap so a caught-up client never grows _selectedLogs unbounded.
    private const int MaxDisplayedLogLines = 5000;
    private static readonly string IconPath = System.IO.Path.Combine(AppContext.BaseDirectory, "icon.ico");
    private string _logFilter = "";
    /// Row widgets by app id, kept alive so RefreshNow can patch rows in place.
    private readonly Dictionary<string, RowWidgets> _rowWidgets = [];
    /// App id order from the last full rebuild, to detect if we can patch instead.
    private List<string> _lastRowOrder = [];
    /// Skips RebuildTrayMenu's work when nothing it shows has actually changed.
    private List<(string Id, bool Active, string StatusLabel, string Name)> _lastTrayMenuState = [];
    /// Character length of each currently-rendered line, in order — lets RenderLogs
    /// delete an exact trimmed prefix instead of falling back to a full rebuild.
    /// Only meaningful when _logRenderedUnfiltered is true.
    private readonly List<int> _renderedLineLengths = [];
    private bool _logRenderedUnfiltered;

    public MainWindow()
    {
        InitializeComponent();
        SystemBackdrop = new MicaBackdrop();
        SetupTrayIcon();

        _timer.Interval = TimeSpan.FromMilliseconds(200);
        _timer.Tick += (_, _) => Refresh();
        _timer.Start();

        // .NET finalizers aren't reliable at process exit — stop child processes explicitly.
        // Shutdown waits for the processes to exit (no orphans holding ports) and closes the CLI channel.
        Closed += (_, _) => { _engine.Shutdown(); _trayIcon?.Dispose(); };

        RefreshNow();
    }

    /// <summary>WinUI3 (app non packagee) n'a pas de tray icon native ; NotifyIcon (WinForms) est l'approche standard.</summary>
    private void SetupTrayIcon()
    {
        try
        {
            _trayIcon = new WinForms.NotifyIcon
            {
                Icon = File.Exists(IconPath) ? new System.Drawing.Icon(IconPath) : System.Drawing.SystemIcons.Application,
                Text = "Switchboard",
                Visible = true,
            };
            RebuildTrayMenu();
            _trayIcon.DoubleClick += (_, _) => this.Activate();
        }
        catch
        {
            // Best-effort : l'app reste utilisable sans icone de tray.
        }
    }

    private void InitPicker(object picker) =>
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(this));

    private void RebuildTrayMenu()
    {
        if (_trayIcon is null) return;
        var state = _apps.Select(a => (a.Id, a.Active, a.StatusLabel, a.Name)).ToList();
        if (state.SequenceEqual(_lastTrayMenuState))
        {
            return;
        }
        _lastTrayMenuState = state;
        var menu = new WinForms.ContextMenuStrip();
        foreach (var app in _apps)
        {
            var item = new WinForms.ToolStripMenuItem($"{(app.Active ? "■" : "▶")}  {app.Name} — {app.StatusLabel}");
            item.Click += (_, _) =>
            {
                if (app.Active) _engine.StopApp(app.Id); else _engine.StartApp(app.Id);
                RefreshNow();
            };
            menu.Items.Add(item);
        }
        menu.Items.Add(new WinForms.ToolStripSeparator());
        var startAll = new WinForms.ToolStripMenuItem("Tout démarrer");
        startAll.Click += (_, _) => { _engine.StartAll(); RefreshNow(); };
        var stopAll = new WinForms.ToolStripMenuItem("Tout arrêter");
        stopAll.Click += (_, _) => { _engine.StopAll(); RefreshNow(); };
        menu.Items.Add(startAll);
        menu.Items.Add(stopAll);
        menu.Items.Add(new WinForms.ToolStripSeparator());
        var quit = new WinForms.ToolStripMenuItem("Quitter Switchboard");
        quit.Click += (_, _) => Microsoft.UI.Xaml.Application.Current.Exit();
        menu.Items.Add(quit);
        _trayIcon.ContextMenuStrip = menu;
    }

    private AppEntry? FindApp(string? id) => _apps.FirstOrDefault(a => a.Id == id);

    private void RefreshNow()
    {
        // Set when RenderLogs can append without a full rebuild.
        (int Trimmed, List<string> NewLines)? incrementalAppend = null;

        _apps = _engine.ListApps(_selectedId, _sinceSeq);
        var selected = FindApp(_selectedId);
        if (selected is { } view)
        {
            if (view.LogsReplace)
            {
                _selectedLogs.Clear();
                _selectedLogs.AddRange(view.Logs);
            }
            else if (view.Logs.Count > 0)
            {
                // If we were empty, LogEditBox still shows the placeholder — needs a full render.
                var wasEmpty = _selectedLogs.Count == 0;
                _selectedLogs.AddRange(view.Logs);
                var overflow = _selectedLogs.Count - MaxDisplayedLogLines;
                if (overflow > 0)
                {
                    _selectedLogs.RemoveRange(0, overflow);
                }
                if (!wasEmpty)
                {
                    incrementalAppend = (Math.Max(overflow, 0), view.Logs);
                }
            }
            _sinceSeq = view.LogsBaseSeq + (ulong)view.Logs.Count;
        }
        NotifyNewFailures();
        RebuildTrayMenu();
        if (selected is null)
        {
            // New selection: reset log tracking so a stale _sinceSeq isn't diffed against a different app.
            selected = _apps.FirstOrDefault();
            _selectedId = selected?.Id;
            if (selected is not null)
            {
                _sinceSeq = 0;
                _selectedLogs.Clear();
            }
        }

        var currentOrder = _apps.Select(a => a.Id).ToList();
        var orderUnchanged = _lastRowOrder.SequenceEqual(currentOrder);

        if (orderUnchanged && _rowWidgets.Count > 0)
        {
            foreach (var app in _apps)
            {
                if (_rowWidgets.TryGetValue(app.Id, out var w))
                {
                    UpdateRow(w, app);
                }
            }
        }
        else
        {
            AppListView.Items.Clear();
            _rowWidgets.Clear();
            foreach (var app in _apps)
            {
                var w = BuildRow(app);
                AppListView.Items.Add(w.Container);
                _rowWidgets[app.Id] = w;
            }
            _lastRowOrder = currentOrder;
        }

        if (selected is null)
        {
            DetailTitle.Text = "Switchboard";
            DetailSubtitle.Text = "Aucune app configurée";
            LogEditBox.Document.SetText(TextSetOptions.None, "");
            _logRenderedUnfiltered = false;
            _renderedLineLengths.Clear();
            return;
        }

        DetailTitle.Text = selected.Name;
        DetailSubtitle.Text = selected.Subtitle;
        RenderLogs(incrementalAppend);
    }

    private void Refresh()
    {
        var rev = _engine.Revision();
        if (rev != _lastSeenRevision)
        {
            _lastSeenRevision = rev;
            RefreshNow();
        }
    }

    private void RenderLogs((int Trimmed, List<string> NewLines)? incremental)
    {
        var filterActive = !string.IsNullOrEmpty(_logFilter);
        var document = LogEditBox.Document;

        // _renderedLineLengths only tracks the full unfiltered log, so an exact-length
        // trim (instead of ITextRange's unreliable paragraph counting) needs that too.
        if (!filterActive && _logRenderedUnfiltered && incremental is { } inc && inc.NewLines.Count > 0
            && inc.Trimmed <= _renderedLineLengths.Count)
        {
            if (inc.Trimmed > 0)
            {
                var dropLength = _renderedLineLengths.Take(inc.Trimmed).Sum(l => l + 1);
                document.GetRange(0, dropLength).SetText(TextSetOptions.None, "");
                _renderedLineLengths.RemoveRange(0, inc.Trimmed);
            }
            var end = document.GetRange(int.MaxValue, int.MaxValue);
            end.SetText(TextSetOptions.None, "\n" + string.Join("\n", inc.NewLines));
            end.Collapse(false);
            document.Selection.SetRange(end.StartPosition, end.StartPosition);
            document.Selection.ScrollIntoView(PointOptions.None);
            _renderedLineLengths.AddRange(inc.NewLines.Select(l => l.Length));
            return;
        }

        if (_selectedLogs.Count == 0)
        {
            document.SetText(TextSetOptions.None, "Pas encore de logs. Démarre l'app pour voir sa sortie ici.");
            _logRenderedUnfiltered = false;
            _renderedLineLengths.Clear();
            return;
        }
        var matching = filterActive
            ? _selectedLogs.Where(l => l.Contains(_logFilter, StringComparison.OrdinalIgnoreCase)).ToList()
            : _selectedLogs;
        document.SetText(TextSetOptions.None, string.Join("\n", matching));
        _logRenderedUnfiltered = !filterActive;
        _renderedLineLengths.Clear();
        if (!filterActive)
        {
            _renderedLineLengths.AddRange(matching.Select(l => l.Length));
        }
        document.Selection.SetRange(int.MaxValue, int.MaxValue);
        document.Selection.ScrollIntoView(PointOptions.None);
    }

    private void NotifyNewFailures()
    {
        foreach (var app in _apps)
        {
            _lastStatus.TryGetValue(app.Id, out var previous);
            _lastStatus[app.Id] = app.StatusLabel;
            if (app.StatusLabel == "failed" && previous != "failed")
            {
                var xml = $"""
                    <toast>
                      <visual>
                        <binding template="ToastGeneric">
                          <text>{app.Name} a crashé</text>
                          <text>{app.Error ?? "Le process s'est arrêté de manière inattendue."}</text>
                        </binding>
                      </visual>
                    </toast>
                    """;
                try
                {
                    AppNotificationManager.Default.Show(new AppNotification(xml));
                }
                catch
                {
                    // Best-effort : ne bloque pas l'UI si les notifications ne sont pas disponibles.
                }
            }
        }
    }

    /// CurrentApp backs the open/edit closures so they use current data on a reused row.
    private sealed class RowWidgets
    {
        public required FrameworkElement Container;
        public required Ellipse Dot;
        public required TextBlock NameText;
        public required TextBlock KindText;
        public required TextBlock StatusText;
        public required Button OpenBtn;
        public required Button StartBtn;
        public required Button StopBtn;
        public required AppEntry CurrentApp;
    }

    // Cached brushes — no reason to allocate a new one per row on every refresh.
    private static readonly SolidColorBrush RunningBrush = new(Color.FromArgb(255, 48, 209, 88));
    private static readonly SolidColorBrush BuildingBrush = new(Color.FromArgb(255, 255, 159, 10));
    private static readonly SolidColorBrush FailedBrush = new(Color.FromArgb(255, 255, 69, 58));
    private static readonly SolidColorBrush StoppedBrush = new(Color.FromArgb(255, 142, 142, 147));
    private static readonly SolidColorBrush ErrorTextBrush = new(Colors.OrangeRed);
    private static readonly SolidColorBrush NormalTextBrush = new(Colors.Gray);
    private static readonly SolidColorBrush KindBadgeBrush = new(Colors.Gray) { Opacity = 0.15 };

    private static SolidColorBrush StatusDotBrush(string statusLabel) => statusLabel switch
    {
        "running" => RunningBrush,
        "building" => BuildingBrush,
        "failed" => FailedBrush,
        _ => StoppedBrush,
    };

    // Shared by BuildRow and UpdateRow so the two can't drift apart on what a row shows.
    private static string StatusTextFor(AppEntry app) => app.Error ?? app.Subtitle;
    private static SolidColorBrush StatusTextBrushFor(AppEntry app) => app.Error is not null ? ErrorTextBrush : NormalTextBrush;
    private static Visibility OpenButtonVisibilityFor(AppEntry app) => string.IsNullOrWhiteSpace(app.Url) ? Visibility.Collapsed : Visibility.Visible;

    private RowWidgets BuildRow(AppEntry app)
    {
        var dot = new Ellipse { Width = 9, Height = 9, Fill = StatusDotBrush(app.StatusLabel), Margin = new Thickness(0, 0, 8, 0) };

        var nameText = new TextBlock { Text = app.Name, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, FontSize = 14 };
        var kindText = new TextBlock { Text = app.Kind.Label(), FontSize = 10, FontFamily = new FontFamily("Consolas") };
        var kindBadge = new Border
        {
            Background = KindBadgeBrush,
            CornerRadius = new CornerRadius(5),
            Padding = new Thickness(6, 1, 6, 1),
            Margin = new Thickness(8, 0, 0, 0),
            Child = kindText,
        };

        var topRow = new StackPanel { Orientation = Orientation.Horizontal, VerticalAlignment = VerticalAlignment.Center };
        topRow.Children.Add(dot);
        topRow.Children.Add(nameText);
        topRow.Children.Add(kindBadge);

        var statusText = new TextBlock
        {
            Text = StatusTextFor(app),
            FontSize = 11,
            FontFamily = new FontFamily("Consolas"),
            Foreground = StatusTextBrushFor(app),
            TextWrapping = TextWrapping.Wrap,
        };

        var actions = new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Right };

        // Assigned below once every widget exists — closures capture w, not app; see RowWidgets.CurrentApp.
        RowWidgets w = null!;

        var openBtn = new Button { Content = "", FontFamily = new FontFamily("Segoe MDL2 Assets"), Margin = new Thickness(0, 0, 4, 0) };
        openBtn.Visibility = OpenButtonVisibilityFor(app);
        openBtn.Click += async (_, _) =>
        {
            if (!string.IsNullOrWhiteSpace(w.CurrentApp.Url))
            {
                await Windows.System.Launcher.LaunchUriAsync(new Uri(w.CurrentApp.Url!));
            }
        };
        actions.Children.Add(openBtn);

        var editBtn = new Button { Content = "", FontFamily = new FontFamily("Segoe MDL2 Assets"), Margin = new Thickness(0, 0, 4, 0) };
        editBtn.Click += (_, _) => ShowAppDialog(w.CurrentApp);
        actions.Children.Add(editBtn);

        var id = app.Id;
        var startBtn = new Button { Content = "▶", IsEnabled = !app.Active, Margin = new Thickness(0, 0, 4, 0) };
        startBtn.Click += (_, _) => { _engine.StartApp(id); _selectedId = id; _sinceSeq = 0; _selectedLogs.Clear(); RefreshNow(); };
        actions.Children.Add(startBtn);

        var stopBtn = new Button { Content = "■", IsEnabled = app.Active, Margin = new Thickness(0, 0, 4, 0) };
        stopBtn.Click += (_, _) => { _engine.StopApp(id); RefreshNow(); };
        actions.Children.Add(stopBtn);

        var deleteBtn = new Button { Content = "🗑" };
        deleteBtn.Click += (_, _) => { _engine.RemoveApp(id); if (_selectedId == id) _selectedId = null; RefreshNow(); };
        actions.Children.Add(deleteBtn);

        var bottomRow = new Grid();
        bottomRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        bottomRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(statusText, 0);
        Grid.SetColumn(actions, 1);
        bottomRow.Children.Add(statusText);
        bottomRow.Children.Add(actions);

        var container = new StackPanel { Spacing = 4, Margin = new Thickness(8, 6, 8, 6), Tag = app.Id };
        container.Children.Add(topRow);
        container.Children.Add(bottomRow);

        w = new RowWidgets
        {
            Container = container,
            Dot = dot,
            NameText = nameText,
            KindText = kindText,
            StatusText = statusText,
            OpenBtn = openBtn,
            StartBtn = startBtn,
            StopBtn = stopBtn,
            CurrentApp = app,
        };
        return w;
    }

    /// Must stay in sync with the mutable fields BuildRow sets.
    private void UpdateRow(RowWidgets w, AppEntry app)
    {
        w.CurrentApp = app;
        w.Dot.Fill = StatusDotBrush(app.StatusLabel);
        w.NameText.Text = app.Name;
        w.KindText.Text = app.Kind.Label();
        w.StatusText.Text = StatusTextFor(app);
        w.StatusText.Foreground = StatusTextBrushFor(app);
        w.OpenBtn.Visibility = OpenButtonVisibilityFor(app);
        w.StartBtn.IsEnabled = !app.Active;
        w.StopBtn.IsEnabled = app.Active;
    }

    private void OnAppSelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (AppListView.SelectedItem is FrameworkElement el && el.Tag is string id)
        {
            _selectedId = id;
            _sinceSeq = 0;
            _selectedLogs.Clear();
            RefreshNow();
        }
    }

    private void OnStartAllClicked(object sender, RoutedEventArgs e)
    {
        _engine.StartAll();
        RefreshNow();
    }

    private void OnStopAllClicked(object sender, RoutedEventArgs e)
    {
        _engine.StopAll();
        RefreshNow();
    }

    private void OnClearLogsClicked(object sender, RoutedEventArgs e)
    {
        if (_selectedId is { } id) _engine.ClearLogs(id);
        _sinceSeq = 0;
        _selectedLogs.Clear();
        RefreshNow();
    }

    private async void OnExportLogsClicked(object sender, RoutedEventArgs e)
    {
        if (_selectedId is not { } id) return;
        var app = FindApp(id);
        var picker = new Windows.Storage.Pickers.FileSavePicker
        {
            SuggestedFileName = $"{app?.Name ?? "logs"}.log",
        };
        picker.FileTypeChoices.Add("Fichier log", [".log", ".txt"]);
        InitPicker(picker);

        var file = await picker.PickSaveFileAsync();
        if (file is not null)
        {
            _engine.ExportLogs(id, file.Path);
        }
    }

    private async void OnAboutClicked(object sender, RoutedEventArgs e)
    {
        var headerPanel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
        if (File.Exists(IconPath))
        {
            var image = new Image
            {
                Source = new Microsoft.UI.Xaml.Media.Imaging.BitmapImage(new Uri(IconPath)),
                Width = 44,
                Height = 44,
            };
            headerPanel.Children.Add(new Border { CornerRadius = new CornerRadius(11), Width = 44, Height = 44, Child = image });
        }
        var headerTextPanel = new StackPanel { Spacing = 2 };
        headerTextPanel.Children.Add(new TextBlock { Text = "Switchboard", FontSize = 18, FontWeight = Microsoft.UI.Text.FontWeights.Bold });
        headerTextPanel.Children.Add(new TextBlock { Text = "Version 0.5.0", FontSize = 12, Opacity = 0.6 });
        headerPanel.Children.Add(headerTextPanel);

        var introText = new TextBlock
        {
            Text = "Démarre, supervise et orchestre tes process de dev locaux — quel que soit le langage.",
            TextWrapping = TextWrapping.Wrap,
            FontSize = 13,
            Opacity = 0.75,
        };

        var linksCard = MakeSectionCard(
            "Liens",
            MakeAboutLinkRow("", "Développé par SkollN", "skolln.com", "https://www.skolln.com"),
            MakeAboutLinkRow("", "Découvre aussi Alume", "Agrégateur de contenus avec IA intégrée", "https://alume.skolln.com"),
            MakeAboutLinkRow("", "Code source", "Open source sous licence GPLv3", "https://github.com/masmarino/switchboard"));

        var panel = new StackPanel { Spacing = 20, Width = 340 };
        panel.Children.Add(headerPanel);
        panel.Children.Add(introText);
        panel.Children.Add(linksCard);

        var dialog = new ContentDialog
        {
            // No Title: the custom header inside Content replaces the default dialog title bar,
            // mirroring the add/edit app dialog's header treatment.
            Content = panel,
            CloseButtonText = "Fermer",
            XamlRoot = Content.XamlRoot,
        };
        await dialog.ShowAsync();
    }

    // HyperlinkButton, not a plain Button, for its built-in URI-launch behavior.
    private static HyperlinkButton MakeAboutLinkRow(string glyph, string title, string subtitle, string uri)
    {
        var icon = new FontIcon { Glyph = glyph, FontFamily = new FontFamily("Segoe MDL2 Assets"), FontSize = 15, Foreground = SectionTitleBrush };
        var textStack = new StackPanel { Spacing = 1 };
        textStack.Children.Add(new TextBlock { Text = title, FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        textStack.Children.Add(new TextBlock { Text = subtitle, FontSize = 11, Opacity = 0.6 });
        var chevron = new FontIcon { Glyph = "", FontFamily = new FontFamily("Segoe MDL2 Assets"), FontSize = 10, Opacity = 0.5 };

        var grid = new Grid { ColumnSpacing = 10 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(icon, 0);
        Grid.SetColumn(textStack, 1);
        Grid.SetColumn(chevron, 2);
        grid.Children.Add(icon);
        grid.Children.Add(textStack);
        grid.Children.Add(chevron);

        return new HyperlinkButton
        {
            NavigateUri = new Uri(uri),
            Content = grid,
            Padding = new Thickness(6),
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            HorizontalAlignment = HorizontalAlignment.Stretch,
        };
    }

    private async void OnExportConfigClicked(object sender, RoutedEventArgs e)
    {
        var checkboxes = new List<(string Id, CheckBox Box)>();
        var appsGroup = new StackPanel { Spacing = 6 };
        foreach (var app in _apps)
        {
            var box = new CheckBox { Content = app.Name, IsChecked = true };
            checkboxes.Add((app.Id, box));
            appsGroup.Children.Add(box);
        }
        var includeEnvVarsBox = new CheckBox { Content = "Inclure les variables d'environnement", IsChecked = false };

        var panel = new StackPanel { Spacing = 20, Width = 340 };
        panel.Children.Add(MakeSectionCard("Apps à exporter", appsGroup));
        panel.Children.Add(MakeSectionCard("Options", includeEnvVarsBox));

        var dialog = new ContentDialog
        {
            Title = "Exporter la config",
            Content = new ScrollViewer { Content = panel, MaxHeight = 480 },
            PrimaryButtonText = "Exporter…",
            CloseButtonText = "Annuler",
            XamlRoot = Content.XamlRoot,
        };

        if (await dialog.ShowAsync() != ContentDialogResult.Primary)
        {
            return;
        }

        var selectedIds = checkboxes.Where(c => c.Box.IsChecked == true).Select(c => c.Id).ToList();
        if (selectedIds.Count == 0)
        {
            return;
        }

        var json = _engine.ExportConfig(selectedIds, includeEnvVarsBox.IsChecked == true);

        var savePicker = new Windows.Storage.Pickers.FileSavePicker();
        savePicker.FileTypeChoices.Add("Configuration JSON", new List<string> { ".json" });
        savePicker.SuggestedFileName = "switchboard-config";
        InitPicker(savePicker);
        var file = await savePicker.PickSaveFileAsync();
        if (file is not null)
        {
            await Windows.Storage.FileIO.WriteTextAsync(file, json);
        }
    }

    private async void OnImportConfigClicked(object sender, RoutedEventArgs e)
    {
        var openPicker = new Windows.Storage.Pickers.FileOpenPicker();
        openPicker.FileTypeFilter.Add(".json");
        InitPicker(openPicker);
        var file = await openPicker.PickSingleFileAsync();
        if (file is null)
        {
            return;
        }

        var json = await Windows.Storage.FileIO.ReadTextAsync(file);
        var preview = _engine.PreviewImportConfig(json);
        if (preview is null)
        {
            await ShowMessageDialog("Fichier invalide", "Ce fichier ne contient pas une configuration Switchboard valide.");
            return;
        }
        if (preview.ToAdd.Count == 0 && preview.ToReplace.Count == 0)
        {
            await ShowMessageDialog("Rien à importer", "Ce fichier ne contient aucune app à ajouter ou remplacer.");
            return;
        }

        var lines = new List<string>();
        if (preview.ToAdd.Count > 0)
        {
            lines.Add($"{preview.ToAdd.Count} app(s) seront ajoutées : {string.Join(", ", preview.ToAdd)}");
        }
        if (preview.ToReplace.Count > 0)
        {
            lines.Add($"{preview.ToReplace.Count} app(s) seront remplacées : {string.Join(", ", preview.ToReplace)}");
        }

        var confirmDialog = new ContentDialog
        {
            Title = "Importer cette configuration ?",
            Content = string.Join("\n", lines),
            PrimaryButtonText = "Importer",
            CloseButtonText = "Annuler",
            XamlRoot = Content.XamlRoot,
        };
        if (await confirmDialog.ShowAsync() == ContentDialogResult.Primary)
        {
            _engine.ApplyImportConfig(json);
            RefreshNow();
        }
    }

    private async Task ShowMessageDialog(string title, string message)
    {
        var dialog = new ContentDialog
        {
            Title = title,
            Content = message,
            CloseButtonText = "OK",
            XamlRoot = Content.XamlRoot,
        };
        await dialog.ShowAsync();
    }

    private void OnLogFilterChanged(object sender, TextChangedEventArgs e)
    {
        _logFilter = LogFilterBox.Text;
        RefreshNow();
    }

    private void OnAddAppClicked(object sender, RoutedEventArgs e) => ShowAppDialog(null);

    private async void ShowAppDialog(AppEntry? existing)
    {
        var nameBox = new TextBox { PlaceholderText = "Nom", Text = existing?.Name ?? "" };
        var dirBox = new TextBox { PlaceholderText = "Dossier", Text = existing?.WorkingDir ?? "" };
        var browseDirBtn = new Button { Content = "Parcourir…", Margin = new Thickness(8, 0, 0, 0) };
        browseDirBtn.Click += async (_, _) =>
        {
            var folderPicker = new Windows.Storage.Pickers.FolderPicker();
            InitPicker(folderPicker);
            var folder = await folderPicker.PickSingleFolderAsync();
            if (folder is not null)
            {
                dirBox.Text = folder.Path;
            }
        };
        var dirRow = new Grid();
        dirRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        dirRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(dirBox, 0);
        Grid.SetColumn(browseDirBtn, 1);
        dirRow.Children.Add(dirBox);
        dirRow.Children.Add(browseDirBtn);
        var kindCombo = new ComboBox { ItemsSource = KindOptions.Select(k => k.Label).ToArray(), HorizontalAlignment = HorizontalAlignment.Stretch };
        kindCombo.SelectedIndex = Math.Max(0, Array.FindIndex(KindOptions, k => k.Kind == (existing?.Kind ?? AppKind.Cargo)));
        var commandBox = new TextBox { PlaceholderText = "Commande (npm/raw)", Text = existing?.Command ?? "" };
        var urlBox = new TextBox { PlaceholderText = "http://localhost:3000 (optionnel)", Text = existing?.Url ?? "" };
        var autoRestartToggle = new ToggleSwitch { IsOn = existing?.AutoRestart ?? false, OnContent = "", OffContent = "" };
        var startOrderBox = new NumberBox
        {
            Value = existing?.StartOrder ?? 0,
            Minimum = 0,
            Maximum = 99,
            SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Inline,
        };
        var envVarsBox = new TextBox
        {
            PlaceholderText = "CLE=valeur (une par ligne)",
            Text = existing?.EnvVarsText ?? "",
            AcceptsReturn = true,
            Height = 80,
            TextWrapping = TextWrapping.Wrap,
        };

        var generalCard = MakeSectionCard(
            "Général",
            MakeFieldRow("Nom", nameBox),
            MakeFieldRow("Dossier", dirRow),
            MakeFieldRow("Type", kindCombo),
            MakeFieldRow("Commande", commandBox));
        var execCard = MakeSectionCard(
            "Exécution",
            MakeFieldRow("URL", urlBox),
            MakeFieldRow("Auto-restart", autoRestartToggle),
            MakeFieldRow("Ordre de démarrage", startOrderBox));
        var advancedCard = MakeSectionCard(
            "Avancé",
            MakeFieldRow("Variables d'env", envVarsBox));

        var headerIcon = new Border
        {
            Width = 36,
            Height = 36,
            CornerRadius = new CornerRadius(18),
            Background = SectionTitleBrush,
            Child = new TextBlock
            {
                Text = existing is not null ? "✎" : "+",
                Foreground = new SolidColorBrush(Colors.White),
                FontSize = 16,
                FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                HorizontalAlignment = HorizontalAlignment.Center,
                VerticalAlignment = VerticalAlignment.Center,
            },
        };
        var headerTextPanel = new StackPanel { Spacing = 2 };
        headerTextPanel.Children.Add(new TextBlock
        {
            Text = existing is not null ? "Modifier l'app" : "Ajouter une app",
            FontSize = 18,
            FontWeight = Microsoft.UI.Text.FontWeights.Bold,
        });
        headerTextPanel.Children.Add(new TextBlock
        {
            Text = existing is not null ? "Mets à jour la configuration de cette app" : "Configure une nouvelle app à superviser",
            FontSize = 12,
            Opacity = 0.6,
        });
        var headerPanel = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12, Margin = new Thickness(0, 0, 0, 4) };
        headerPanel.Children.Add(headerIcon);
        headerPanel.Children.Add(headerTextPanel);

        var errorsText = new TextBlock { TextWrapping = TextWrapping.Wrap, Foreground = new SolidColorBrush(Colors.Firebrick), Visibility = Visibility.Collapsed };
        var warningsText = new TextBlock { TextWrapping = TextWrapping.Wrap, Foreground = new SolidColorBrush(Colors.DarkOrange), Visibility = Visibility.Collapsed };

        var panel = new StackPanel { Spacing = 20 };
        panel.Children.Add(headerPanel);
        panel.Children.Add(errorsText);
        panel.Children.Add(warningsText);
        panel.Children.Add(generalCard);
        panel.Children.Add(execCard);
        panel.Children.Add(advancedCard);

        var dialog = new ContentDialog
        {
            // No Title: the custom header inside Content replaces the default dialog title bar.
            Content = new ScrollViewer { Content = panel, MaxHeight = 560 },
            PrimaryButtonText = existing is not null ? "Enregistrer" : "Ajouter",
            CloseButtonText = "Annuler",
            XamlRoot = Content.XamlRoot,
        };

        AppDraftPayload BuildDraft() => new()
        {
            Name = nameBox.Text.Trim(),
            WorkingDir = dirBox.Text.Trim(),
            Kind = KindOptions[Math.Max(0, kindCombo.SelectedIndex)].Kind.ToFfiValue(),
            Command = commandBox.Text.Trim(),
            Url = string.IsNullOrWhiteSpace(urlBox.Text) ? null : urlBox.Text.Trim(),
            EnvVars = AppEntry.ParseEnvVarsText(envVarsBox.Text),
            AutoRestart = autoRestartToggle.IsOn,
            StartOrder = double.IsNaN(startOrderBox.Value) ? 0 : (int)startOrderBox.Value,
        };

        // Errors keep the dialog open; warnings need a second click to save anyway.
        var warned = false;
        dialog.PrimaryButtonClick += (_, args) =>
        {
            var report = _engine.Validate(BuildDraft(), existing?.Id) ?? new DraftValidation();
            var warnings = report.Warnings.Where(w => w.Code != "ignored").ToList();
            errorsText.Text = string.Join("\n", report.Errors.Select(e => $"• {e.LocalizedMessage}"));
            errorsText.Visibility = report.Errors.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
            warningsText.Text = string.Join("\n", warnings.Select(w => $"• {w.LocalizedMessage}"));
            warningsText.Visibility = warnings.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
            if (report.Errors.Count > 0)
            {
                warned = false;
                args.Cancel = true;
            }
            else if (warnings.Count > 0 && !warned)
            {
                warned = true;
                dialog.PrimaryButtonText = "Enregistrer quand même";
                args.Cancel = true;
            }
        };

        if (await dialog.ShowAsync() == ContentDialogResult.Primary)
        {
            var draft = BuildDraft();

            if (existing is not null)
            {
                _engine.UpdateApp(existing.Id, draft);
            }
            else
            {
                _engine.AddApp(draft);
            }
            RefreshNow();
        }
    }

    private static readonly SolidColorBrush SectionTitleBrush = new(Color.FromArgb(255, 0x04, 0x09, 0x43));

    private static StackPanel MakeSectionCard(string title, params FrameworkElement[] rows)
    {
        var titleBlock = new TextBlock
        {
            Text = title.ToUpperInvariant(),
            FontSize = 11,
            FontWeight = Microsoft.UI.Text.FontWeights.Bold,
            Foreground = SectionTitleBrush,
            Margin = new Thickness(0, 0, 0, 8),
        };

        var rowsPanel = new StackPanel { Spacing = 12 };
        foreach (var row in rows)
        {
            rowsPanel.Children.Add(row);
        }

        var card = new Border
        {
            Background = new SolidColorBrush(Colors.Gray) { Opacity = 0.08 },
            CornerRadius = new CornerRadius(10),
            Padding = new Thickness(14),
            Child = rowsPanel,
        };

        var container = new StackPanel { Spacing = 8 };
        container.Children.Add(titleBlock);
        container.Children.Add(card);
        return container;
    }

    /// <summary>Fixed-width label column so rows align regardless of label length.</summary>
    private static Grid MakeFieldRow(string label, FrameworkElement control)
    {
        var grid = new Grid();
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(150) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        var labelBlock = new TextBlock { Text = label, VerticalAlignment = VerticalAlignment.Center };
        Grid.SetColumn(labelBlock, 0);
        Grid.SetColumn(control, 1);
        grid.Children.Add(labelBlock);
        grid.Children.Add(control);
        return grid;
    }
}
