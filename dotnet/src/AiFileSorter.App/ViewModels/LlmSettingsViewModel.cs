using System.Collections.ObjectModel;
using System.Linq;
using AiFileSorter.Core.Llm;
using AiFileSorter.Core.Models;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace AiFileSorter.App.ViewModels;

public sealed partial class LlmSettingsViewModel : ObservableObject
{
    public GgufDownloader Downloader { get; } = new();

    [ObservableProperty]
    private LlmKind kind = LlmKind.Heuristic;

    [ObservableProperty]
    private string openAiApiKey = "";

    [ObservableProperty]
    private string openAiModel = "gpt-4.1-mini";

    [ObservableProperty]
    private string geminiApiKey = "";

    [ObservableProperty]
    private string geminiModel = "gemini-2.5-flash";

    [ObservableProperty]
    private string customName = "";

    [ObservableProperty]
    private string customBaseUrl = "";

    [ObservableProperty]
    private string customApiKey = "";

    [ObservableProperty]
    private string customModel = "";

    [ObservableProperty]
    private string localGgufPath = "";

    [ObservableProperty]
    private string localMmprojPath = "";

    [ObservableProperty]
    private string modelStorageDir = GgufStorage.DefaultDirectory();

    [ObservableProperty]
    private string visualBackendId = "gemma-3-4b-it";

    [ObservableProperty]
    private string builtinLocalModelId = "gemma-3-4b-it";

    public ObservableCollection<GgufDownloadItemViewModel> CategorizationDownloads { get; } = [];
    public ObservableCollection<GgufDownloadItemViewModel> VisualDownloads { get; } = [];
    public IReadOnlyList<GgufVisualBackend> VisualBackendChoices => GgufCatalog.VisualBackends;

    public GgufVisualBackend? SelectedVisualBackendChoice
    {
        get => GgufCatalog.FindVisualBackend(VisualBackendId);
        set
        {
            if (value is not null)
            {
                VisualBackendId = value.Id;
            }
        }
    }

    public int SelectedKindIndex
    {
        get => (int)Kind;
        set => Kind = (LlmKind)value;
    }

    public string Summary => LlmCatalog.Summarize(ToSettings());
    public bool ShowLocalDownloads => Kind == LlmKind.LocalGguf;

    public LlmSettingsViewModel()
    {
        foreach (var model in GgufCatalog.CategorizationModels)
        {
            CategorizationDownloads.Add(new GgufDownloadItemViewModel(model.Artifact, this));
        }

        RebuildVisualDownloads();
    }

    public void Load(LlmEndpointSettings settings)
    {
        Kind = settings.Kind;
        OpenAiApiKey = settings.OpenAiApiKey;
        OpenAiModel = settings.OpenAiModel;
        GeminiApiKey = settings.GeminiApiKey;
        GeminiModel = settings.GeminiModel;
        CustomName = settings.CustomName;
        CustomBaseUrl = settings.CustomBaseUrl;
        CustomApiKey = settings.CustomApiKey;
        CustomModel = settings.CustomModel;
        LocalGgufPath = settings.LocalGgufPath;
        LocalMmprojPath = settings.LocalMmprojPath;
        ModelStorageDir = string.IsNullOrWhiteSpace(settings.ModelStorageDir)
            ? GgufStorage.DefaultDirectory()
            : settings.ModelStorageDir;
        VisualBackendId = settings.VisualBackendId;
        BuiltinLocalModelId = settings.BuiltinLocalModelId;
        RefreshDownloads();
        NotifySummary();
    }

    public LlmEndpointSettings ToSettings()
    {
        var resolved = GgufCatalog.ResolveCategorizationModelPath(new LlmEndpointSettings
        {
            BuiltinLocalModelId = BuiltinLocalModelId,
            ModelStorageDir = ModelStorageDir,
            LocalGgufPath = LocalGgufPath
        });
        var visual = GgufCatalog.ResolveVisualPaths(new LlmEndpointSettings
        {
            VisualBackendId = VisualBackendId,
            ModelStorageDir = ModelStorageDir,
            LocalGgufPath = LocalGgufPath,
            LocalMmprojPath = LocalMmprojPath
        });
        return new LlmEndpointSettings
        {
            Kind = Kind,
            OpenAiApiKey = OpenAiApiKey,
            OpenAiModel = OpenAiModel,
            GeminiApiKey = GeminiApiKey,
            GeminiModel = GeminiModel,
            CustomName = CustomName,
            CustomBaseUrl = CustomBaseUrl,
            CustomApiKey = CustomApiKey,
            CustomModel = CustomModel,
            LocalGgufPath = resolved,
            LocalMmprojPath = visual.MmprojPath ?? "",
            ModelStorageDir = ModelStorageDir,
            VisualBackendId = VisualBackendId,
            BuiltinLocalModelId = BuiltinLocalModelId
        };
    }

    public void OnDownloadFinished()
    {
        LocalGgufPath = GgufCatalog.ResolveCategorizationModelPath(ToSettings());
        LocalMmprojPath = GgufCatalog.ResolveVisualPaths(ToSettings()).MmprojPath ?? "";
        NotifySummary();
        RefreshDownloads();
    }

    public void RefreshDownloads()
    {
        foreach (var item in CategorizationDownloads)
        {
            item.Refresh();
        }

        foreach (var item in VisualDownloads)
        {
            item.Refresh();
        }
    }

    public void SelectModel(string id)
    {
        BuiltinLocalModelId = id;
        Kind = LlmKind.LocalGguf;
        OnDownloadFinished();
    }

    [RelayCommand]
    private void SelectCategorizationModel(string? id)
    {
        if (!string.IsNullOrWhiteSpace(id))
        {
            SelectModel(id);
        }
    }

    [RelayCommand]
    private void ResetStorageDir()
    {
        ModelStorageDir = GgufStorage.DefaultDirectory();
    }

    public void SetStorageDir(string path)
    {
        ModelStorageDir = path;
        RefreshDownloads();
        NotifySummary();
    }

    partial void OnKindChanged(LlmKind value)
    {
        OnPropertyChanged(nameof(SelectedKindIndex));
        OnPropertyChanged(nameof(ShowLocalDownloads));
        NotifySummary();
    }

    partial void OnBuiltinLocalModelIdChanged(string value)
    {
        foreach (var item in CategorizationDownloads)
        {
            item.NotifySelected();
        }

        NotifySummary();
    }

    partial void OnVisualBackendIdChanged(string value)
    {
        RebuildVisualDownloads();
        OnPropertyChanged(nameof(SelectedVisualBackendChoice));
        NotifySummary();
    }

    partial void OnModelStorageDirChanged(string value) => RefreshDownloads();

    private void RebuildVisualDownloads()
    {
        VisualDownloads.Clear();
        var backend = GgufCatalog.FindVisualBackend(VisualBackendId) ?? GgufCatalog.VisualBackends[0];
        VisualDownloads.Add(new GgufDownloadItemViewModel(backend.TextModel, this));
        VisualDownloads.Add(new GgufDownloadItemViewModel(backend.Mmproj, this));
    }

    private void NotifySummary() => OnPropertyChanged(nameof(Summary));
}
