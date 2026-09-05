using System.Collections.ObjectModel;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using AiFileSorter.Core.Engine;
using AiFileSorter.Core.Json;
using AiFileSorter.Core.Models;
using AiFileSorter.Core.Plans;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace AiFileSorter.App.ViewModels;

public partial class MainWindowViewModel : ObservableObject
{
    private CancellationTokenSource? _analysisCts;

    [ObservableProperty]
    private string folderPath = "";

    [ObservableProperty]
    private bool recursive = true;

    [ObservableProperty]
    private bool analyzeMediaContent = true;

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

    public ObservableCollection<CategorizedItem> Entries { get; } = [];
    public ObservableCollection<ArchiveEntitySuggestion> ArchiveEntities { get; } = [];
    public ObservableCollection<CategorizedItem> LockedItems { get; } = [];

    public string EngineMode => ProcessEngineClient.FindExecutable() is null
        ? "In-process worker thread"
        : "Isolated aifs engine process";

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

        var request = new AnalysisRequest
        {
            RootPath = FolderPath,
            Scan = new ScanOptions
            {
                Flags = ScanFlags.Files | ScanFlags.Directories |
                        (Recursive ? ScanFlags.Recursive : ScanFlags.None)
            },
            Style = ConsistentStyle ? CategorizationStyle.Consistent : CategorizationStyle.Refined,
            AnalyzeMediaContent = AnalyzeMediaContent,
            DatabasePath = AnalysisEngine.DefaultDatabasePath()
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
            Status = $"Ready. {plan.Summary.FileCount} files, {plan.Summary.ArchiveEntityCount} project archives, {plan.Summary.LockedCount} locked.";
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
        await File.WriteAllTextAsync(path, AppJson.Serialize(CurrentPlan)).ConfigureAwait(true);
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

        var payload = new RemotePlanHandoff().Create(CurrentPlan);
        var jsonPath = Path.Combine(FolderPath, "aifs-remote-handoff.json");
        var promptPath = Path.Combine(FolderPath, "aifs-remote-handoff.txt");
        await File.WriteAllTextAsync(jsonPath, payload.CompactJson).ConfigureAwait(true);
        await File.WriteAllTextAsync(promptPath, payload.CompactPrompt).ConfigureAwait(true);
        Status = $"Wrote remote handoff files for a more powerful model: {jsonPath}";
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

    private void ApplyPlan(FilingPlan plan)
    {
        CurrentPlan = plan;
        Entries.Clear();
        ArchiveEntities.Clear();
        LockedItems.Clear();
        foreach (var entry in plan.Entries)
        {
            Entries.Add(entry);
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
}
