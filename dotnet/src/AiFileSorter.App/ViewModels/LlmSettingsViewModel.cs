using System.Linq;
using AiFileSorter.Core.Llm;
using AiFileSorter.Core.Models;
using CommunityToolkit.Mvvm.ComponentModel;

namespace AiFileSorter.App.ViewModels;

public sealed partial class LlmSettingsViewModel : ObservableObject
{
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
    private string modelStorageDir = "";

    [ObservableProperty]
    private string visualBackendId = "gemma-3-4b-it";

    [ObservableProperty]
    private string builtinLocalModelId = "gemma-3-4b-it";

    public IReadOnlyList<LlmCatalogEntry> RemoteEndpoints => LlmCatalog.RemoteEndpoints;
    public IReadOnlyList<LlmCatalogEntry> BuiltinLocalModels => LlmCatalog.BuiltinLocalModels;
    public IReadOnlyList<LlmCatalogEntry> VisualBackends => LlmCatalog.VisualBackends;

    public LlmCatalogEntry? SelectedLocalModel
    {
        get => BuiltinLocalModels.FirstOrDefault(entry => entry.Id == BuiltinLocalModelId);
        set
        {
            if (value is not null)
            {
                BuiltinLocalModelId = value.Id;
            }
        }
    }

    public LlmCatalogEntry? SelectedVisualBackend
    {
        get => VisualBackends.FirstOrDefault(entry => entry.Id == VisualBackendId);
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
        ModelStorageDir = settings.ModelStorageDir;
        VisualBackendId = settings.VisualBackendId;
        BuiltinLocalModelId = settings.BuiltinLocalModelId;
        OnPropertyChanged(nameof(Summary));
        OnPropertyChanged(nameof(SelectedLocalModel));
        OnPropertyChanged(nameof(SelectedVisualBackend));
    }

    public LlmEndpointSettings ToSettings() => new()
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
        LocalGgufPath = LocalGgufPath,
        LocalMmprojPath = LocalMmprojPath,
        ModelStorageDir = ModelStorageDir,
        VisualBackendId = VisualBackendId,
        BuiltinLocalModelId = BuiltinLocalModelId
    };

    partial void OnKindChanged(LlmKind value)
    {
        OnPropertyChanged(nameof(Summary));
        OnPropertyChanged(nameof(SelectedKindIndex));
    }
    partial void OnBuiltinLocalModelIdChanged(string value) => OnPropertyChanged(nameof(SelectedLocalModel));
    partial void OnVisualBackendIdChanged(string value) => OnPropertyChanged(nameof(SelectedVisualBackend));
}
