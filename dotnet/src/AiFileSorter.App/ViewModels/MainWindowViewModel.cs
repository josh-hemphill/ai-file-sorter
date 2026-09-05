using System;
using System.Collections.ObjectModel;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using AiFileSorter.Core.Engine;
using AiFileSorter.Core.Json;
using AiFileSorter.Core.Llm;
using AiFileSorter.Core.Models;
using AiFileSorter.Core.Persistence;
using AiFileSorter.Core.Plans;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace AiFileSorter.App.ViewModels;

public partial class MainWindowViewModel : ObservableObject
{
    private CancellationTokenSource? _analysisCts;
    private string? _sortColumn;
    private bool _sortAscending = true;

    [ObservableProperty]
    private string folderPath = "";

    [ObservableProperty]
    private bool useSubcategories = true;

    [ObservableProperty]
    private bool categorizeFiles = true;

    [ObservableProperty]
    private bool categorizeDirectories = true;

    [ObservableProperty]
    private bool includeSubdirectories = true;

    [ObservableProperty]
    private bool analyzeDocuments;

    [ObservableProperty]
    private bool analyzeImages;

    [ObservableProperty]
    private bool analyzeMedia = true;

    [ObservableProperty]
    private bool processDocumentsOnly;

    [ObservableProperty]
    private bool processImagesOnly;

    [ObservableProperty]
    private bool offerRenameDocuments;

    [ObservableProperty]
    private bool offerRenameImages;

    [ObservableProperty]
    private bool offerRenameMedia = true;

    [ObservableProperty]
    private bool renameDocumentsOnly;

    [ObservableProperty]
    private bool renameImagesOnly;

    [ObservableProperty]
    private bool addDocumentDateToCategory;

    [ObservableProperty]
    private bool addImageDateToCategory;

    [ObservableProperty]
    private bool addImageDatePlaceToFilename;

    [ObservableProperty]
    private bool useWhitelist;

    [ObservableProperty]
    private string activeWhitelist = "Unrestricted";

    [ObservableProperty]
    private bool consistentStyle;

    [ObservableProperty]
    private bool isAnalyzing;

    [ObservableProperty]
    private string status = "Select a folder. Analysis never runs on the UI thread.";

    [ObservableProperty]
    private string progressText = "";

    [ObservableProperty]
    private double progressFraction;

    [ObservableProperty]
    private FilingPlan? currentPlan;

    [ObservableProperty]
    private string modelSummary = LlmCatalog.Summarize(new LlmEndpointSettings());

    public LlmSettingsViewModel Llm { get; } = new();

    public IReadOnlyList<string> WhitelistNames => LlmCatalog.WhitelistNames;

    public ObservableCollection<SuggestionRowViewModel> Rows { get; } = [];
    public ObservableCollection<ArchiveEntitySuggestion> ArchiveEntities { get; } = [];
    public ObservableCollection<CategorizedItem> LockedItems { get; } = [];

    public string EngineMode => ProcessEngineClient.FindExecutable() is null
        ? "In-process worker thread"
        : "Isolated aifs engine process";

    public MainWindowViewModel()
    {
        var settings = SettingsStore.Load();
        Llm.Load(settings.Llm);
        ApplyContentSettings(settings.Content);
        ModelSummary = Llm.Summary;
    }

    [RelayCommand(CanExecute = nameof(CanAnalyze))]
    private async Task AnalyzeAsync()
    {
        if (string.IsNullOrWhiteSpace(FolderPath) || !Directory.Exists(FolderPath))
        {
            Status = "Choose an existing folder first.";
            return;
        }

        _analysisCts?.Cancel();
        _analysisCts?.Dispose();
        _analysisCts = new CancellationTokenSource();
        var token = _analysisCts.Token;

        IsAnalyzing = true;
        ProgressFraction = 0;
        ProgressText = "Starting…";
        Status = "Analyzing off the UI thread…";
        AnalyzeCommand.NotifyCanExecuteChanged();
        CancelAnalyzeCommand.NotifyCanExecuteChanged();
        PersistSettings();

        var request = new AnalysisRequest
        {
            RootPath = FolderPath,
            Scan = new ScanOptions
            {
                Flags = ScanFlags.Files | ScanFlags.Directories |
                        (IncludeSubdirectories ? ScanFlags.Recursive : ScanFlags.None)
            },
            Style = ConsistentStyle ? CategorizationStyle.Consistent : CategorizationStyle.Refined,
            DatabasePath = AnalysisEngine.DefaultDatabasePath(),
            Content = BuildContentOptions(),
            Llm = Llm.ToSettings()
        };

        var progress = new Progress<AnalysisProgress>(update =>
        {
            ProgressFraction = update.Fraction;
            ProgressText = string.IsNullOrWhiteSpace(update.Message)
                ? update.Stage
                : $"{update.Stage}: {update.Message}";
        });

        try
        {
            FilingPlan plan;
            var processClient = ProcessEngineClient.TryStart();
            if (processClient is not null)
            {
                await using (processClient)
                {
                    plan = await processClient.AnalyzeAsync(request, progress, token).ConfigureAwait(true);
                }
            }
            else
            {
                plan = await new AnalysisEngine().AnalyzeAsync(request, progress, token).ConfigureAwait(true);
            }

            ApplyPlan(plan);
            Status = $"Ready. {plan.Summary.FileCount} files, {plan.Summary.ArchiveEntityCount} project archives, {plan.Summary.LockedCount} locked. Saved to SQLite.";
            ProgressText = "Done";
            ProgressFraction = 1;
        }
        catch (OperationCanceledException)
        {
            Status = "Analysis cancelled.";
            ProgressText = "Cancelled";
        }
        catch (Exception ex)
        {
            Status = "Analysis failed: " + ex.Message;
        }
        finally
        {
            IsAnalyzing = false;
            AnalyzeCommand.NotifyCanExecuteChanged();
            CancelAnalyzeCommand.NotifyCanExecuteChanged();
        }
    }

    [RelayCommand(CanExecute = nameof(CanCancel))]
    private void CancelAnalyze()
    {
        _analysisCts?.Cancel();
    }

    [RelayCommand]
    private async Task ExportPlanAsync()
    {
        if (CurrentPlan is null)
        {
            Status = "Analyze a folder before exporting a plan.";
            return;
        }

        var path = Path.Combine(FolderPath, "aifs-filing-plan.json");
        await File.WriteAllTextAsync(path, AppJson.Serialize(PlanFromRows())).ConfigureAwait(true);
        Status = "Wrote " + path;
    }

    [RelayCommand]
    private async Task ExportHandoffAsync()
    {
        if (CurrentPlan is null)
        {
            Status = "Analyze a folder before creating a remote handoff.";
            return;
        }

        var payload = new RemotePlanHandoff().Create(PlanFromRows());
        var jsonPath = Path.Combine(FolderPath, "aifs-remote-handoff.json");
        var promptPath = Path.Combine(FolderPath, "aifs-remote-handoff.txt");
        await File.WriteAllTextAsync(jsonPath, payload.CompactJson).ConfigureAwait(true);
        await File.WriteAllTextAsync(promptPath, payload.CompactPrompt).ConfigureAwait(true);
        Status = $"Wrote remote handoff files. Import the model JSON with Merge remote proposal: {jsonPath}";
    }

    [RelayCommand]
    private async Task AskRemoteAsync()
    {
        if (CurrentPlan is null)
        {
            Status = "Analyze a folder before asking a remote model.";
            return;
        }

        try
        {
            Status = "Asking remote model for path updates…";
            var proposal = await AnalysisEngine.RequestRemoteProposalAsync(
                PlanFromRows(),
                Llm.ToSettings()).ConfigureAwait(true);
            var merged = new AnalysisEngine().MergeRemote(PlanFromRows(), proposal, AnalysisEngine.DefaultDatabasePath());
            ApplyPlan(merged);
            Status = $"Remote proposals saved to the local database. {merged.Entries.Count(entry => entry.Status == SuggestionStatus.RemoteProposed)} rows pending review. Files were not moved.";
        }
        catch (Exception ex)
        {
            Status = "Remote proposal failed: " + ex.Message;
        }
    }

    [RelayCommand]
    private async Task ImportRemoteProposalAsync()
    {
        if (CurrentPlan is null || string.IsNullOrWhiteSpace(FolderPath))
        {
            Status = "Analyze a folder before importing a remote proposal.";
            return;
        }

        var path = Path.Combine(FolderPath, "aifs-remote-proposal.json");
        if (!File.Exists(path))
        {
            Status = "Place a model JSON response at " + path;
            return;
        }

        try
        {
            var json = await File.ReadAllTextAsync(path).ConfigureAwait(true);
            var proposal = RemotePlanHandoff.ParseModelJson(json);
            var merged = new AnalysisEngine().MergeRemote(PlanFromRows(), proposal, AnalysisEngine.DefaultDatabasePath());
            ApplyPlan(merged);
            Status = "Imported remote path proposals into the local database. Review the Proposed path column, then apply locally.";
        }
        catch (Exception ex)
        {
            Status = "Import failed: " + ex.Message;
        }
    }

    [RelayCommand]
    private void AcceptRemoteSelected()
    {
        foreach (var row in Rows.Where(item => item.Selected))
        {
            row.AcceptRemote();
        }

        Status = "Accepted remote paths for selected rows. Nothing was moved yet.";
    }

    [RelayCommand]
    private void KeepLocalSelected()
    {
        foreach (var row in Rows.Where(item => item.Selected))
        {
            row.KeepLocal();
        }

        Status = "Kept local paths for selected rows. Nothing was moved yet.";
    }

    [RelayCommand]
    private void RejectRemoteSelected()
    {
        foreach (var row in Rows.Where(item => item.Selected))
        {
            row.RejectRemote();
        }

        Status = "Rejected remote paths for selected rows.";
    }

    [RelayCommand]
    private void PreviewApply()
    {
        if (CurrentPlan is null)
        {
            Status = "Analyze a folder before applying.";
            return;
        }

        var preview = new AnalysisEngine().PreviewApply(PlanFromRows());
        Status = preview.Moves.Count == 0
            ? "Dry run: no file moves. Accept or edit proposed paths first."
            : $"Dry run: {preview.Moves.Count} local moves, {preview.Warnings.Count} warnings. Files were not changed.";
    }

    [RelayCommand]
    private void ApplyLocally()
    {
        if (CurrentPlan is null)
        {
            Status = "Analyze a folder before applying.";
            return;
        }

        var engine = new AnalysisEngine();
        var result = engine.Apply(PlanFromRows(), dryRun: false, AnalysisEngine.DefaultDatabasePath());
        if (File.Exists(AnalysisEngine.DefaultDatabasePath()))
        {
            using var store = new SuggestionStore(AnalysisEngine.DefaultDatabasePath());
            var updated = store.LoadPlan(CurrentPlan.PlanId) ?? ApplyService.MarkApplied(PlanFromRows(), result);
            ApplyPlan(updated);
        }

        Status = result.Applied.Count == 0
            ? "No files moved. " + string.Join(" ", result.Errors)
            : $"Applied {result.Applied.Count} local moves. Undo plan: {result.UndoPlanPath}";
    }

    [RelayCommand]
    public void SortBy(string column)
    {
        if (_sortColumn == column)
        {
            _sortAscending = !_sortAscending;
        }
        else
        {
            _sortColumn = column;
            _sortAscending = true;
        }

        var ordered = column switch
        {
            "name" => Sort(Rows, row => row.FileName),
            "family" => Sort(Rows, row => row.Family.ToString()),
            "category" => Sort(Rows, row => row.Category),
            "subcategory" => Sort(Rows, row => row.Subcategory),
            "path" => Sort(Rows, row => row.ProposedPath),
            "status" => Sort(Rows, row => row.Status.ToString()),
            _ => Rows.ToArray()
        };

        Rows.Clear();
        foreach (var row in ordered)
        {
            Rows.Add(row);
        }
    }

    public void SaveLlmSettings()
    {
        PersistSettings();
        ModelSummary = Llm.Summary;
        Status = "Saved model settings. Local GGUF inference still runs in the C++ sidecar.";
    }

    public void SetFolder(string path)
    {
        FolderPath = path;
        Status = "Folder selected: " + path;
        AnalyzeCommand.NotifyCanExecuteChanged();
    }

    partial void OnFolderPathChanged(string value) => AnalyzeCommand.NotifyCanExecuteChanged();

    private bool CanAnalyze() => !IsAnalyzing && Directory.Exists(FolderPath);
    private bool CanCancel() => IsAnalyzing;

    public FilingPlan PlanFromRows()
    {
        if (CurrentPlan is null)
        {
            throw new InvalidOperationException("No current plan.");
        }

        return CurrentPlan with { Entries = Rows.Select(row => row.ToItem()).ToArray() };
    }

    public void ApplyPlan(FilingPlan plan)
    {
        CurrentPlan = plan;
        Rows.Clear();
        ArchiveEntities.Clear();
        LockedItems.Clear();
        foreach (var entry in plan.Entries)
        {
            Rows.Add(new SuggestionRowViewModel(entry));
        }

        foreach (var archive in plan.ArchiveEntities)
        {
            ArchiveEntities.Add(archive);
        }

        foreach (var locked in plan.LockedItems)
        {
            LockedItems.Add(locked);
        }
    }

    private ContentAnalysisOptions BuildContentOptions() => new()
    {
        CategorizeFiles = CategorizeFiles,
        CategorizeDirectories = CategorizeDirectories,
        IncludeSubdirectories = IncludeSubdirectories,
        UseSubcategories = UseSubcategories,
        AnalyzeDocuments = AnalyzeDocuments,
        AnalyzeImages = AnalyzeImages,
        AnalyzeMedia = AnalyzeMedia,
        ProcessDocumentsOnly = ProcessDocumentsOnly,
        ProcessImagesOnly = ProcessImagesOnly,
        OfferRenameDocuments = OfferRenameDocuments,
        OfferRenameImages = OfferRenameImages,
        OfferRenameMedia = OfferRenameMedia,
        RenameDocumentsOnly = RenameDocumentsOnly,
        RenameImagesOnly = RenameImagesOnly,
        AddDocumentDateToCategory = AddDocumentDateToCategory,
        AddImageDateToCategory = AddImageDateToCategory,
        AddImageDatePlaceToFilename = AddImageDatePlaceToFilename,
        UseWhitelist = UseWhitelist,
        ActiveWhitelist = ActiveWhitelist
    };

    private void ApplyContentSettings(ContentAnalysisOptions content)
    {
        CategorizeFiles = content.CategorizeFiles;
        CategorizeDirectories = content.CategorizeDirectories;
        IncludeSubdirectories = content.IncludeSubdirectories;
        UseSubcategories = content.UseSubcategories;
        AnalyzeDocuments = content.AnalyzeDocuments;
        AnalyzeImages = content.AnalyzeImages;
        AnalyzeMedia = content.AnalyzeMedia;
        ProcessDocumentsOnly = content.ProcessDocumentsOnly;
        ProcessImagesOnly = content.ProcessImagesOnly;
        OfferRenameDocuments = content.OfferRenameDocuments;
        OfferRenameImages = content.OfferRenameImages;
        OfferRenameMedia = content.OfferRenameMedia;
        RenameDocumentsOnly = content.RenameDocumentsOnly;
        RenameImagesOnly = content.RenameImagesOnly;
        AddDocumentDateToCategory = content.AddDocumentDateToCategory;
        AddImageDateToCategory = content.AddImageDateToCategory;
        AddImageDatePlaceToFilename = content.AddImageDatePlaceToFilename;
        UseWhitelist = content.UseWhitelist;
        ActiveWhitelist = content.ActiveWhitelist;
        ConsistentStyle = false;
    }

    private void PersistSettings()
    {
        SettingsStore.Save(new AppSettings
        {
            Llm = Llm.ToSettings(),
            Content = BuildContentOptions()
        });
    }

    private SuggestionRowViewModel[] Sort(ObservableCollection<SuggestionRowViewModel> rows, Func<SuggestionRowViewModel, string> key)
    {
        var query = rows.OrderBy(key, StringComparer.OrdinalIgnoreCase);
        return (_sortAscending ? query : query.Reverse()).ToArray();
    }
}
