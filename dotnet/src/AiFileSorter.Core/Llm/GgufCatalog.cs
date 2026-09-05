using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Llm;

public enum GgufFunction
{
    Categorization,
    Documents,
    ImageAnalysis
}

public enum GgufArtifactKind
{
    TextModel,
    Mmproj
}

public enum GgufLocalState
{
    MissingUrl,
    NotStarted,
    Partial,
    Complete,
    Corrupt
}

public sealed record GgufArtifact
{
    public required string Id { get; init; }
    public required string DisplayName { get; init; }
    public required string UrlEnv { get; init; }
    public required string DefaultUrl { get; init; }
    public required string RelativePath { get; init; }
    public required GgufArtifactKind Kind { get; init; }
    public IReadOnlyList<GgufFunction> Functions { get; init; } = [];
    public string Notes { get; init; } = "";
    public string? SharesUrlWithArtifactId { get; init; }

    public string ResolveUrl()
    {
        var env = Environment.GetEnvironmentVariable(UrlEnv);
        return string.IsNullOrWhiteSpace(env) ? DefaultUrl : env.Trim();
    }
}

public sealed record GgufModelChoice
{
    public required string Id { get; init; }
    public required string DisplayName { get; init; }
    public required GgufArtifact Artifact { get; init; }
    public bool IsLegacy { get; init; }
}

public sealed record GgufVisualBackend
{
    public required string Id { get; init; }
    public required string DisplayName { get; init; }
    public required GgufArtifact TextModel { get; init; }
    public required GgufArtifact Mmproj { get; init; }
}

/// <summary>Built-in GGUF catalog matching the Qt Select LLM dialog, including download URLs from app/resources/.env.</summary>
public static class GgufCatalog
{
    public static readonly GgufArtifact Gemma3Categorization = new()
    {
        Id = "gemma-3-4b-it",
        DisplayName = "Gemma 3 4B IT Q4_K_M",
        UrlEnv = "LOCAL_LLM_3B_DOWNLOAD_URL",
        DefaultUrl = "https://huggingface.co/ggml-org/gemma-3-4b-it-GGUF/resolve/main/gemma-3-4b-it-Q4_K_M.gguf",
        RelativePath = "gemma-3-4b-it-Q4_K_M.gguf",
        Kind = GgufArtifactKind.TextModel,
        Functions = [GgufFunction.Categorization, GgufFunction.Documents],
        Notes = "Default local model for categorization and document summaries.",
        SharesUrlWithArtifactId = "gemma-3-4b-it-visual-model"
    };

    public static readonly GgufArtifact Mistral7B = new()
    {
        Id = "mistral-7b",
        DisplayName = "Mistral 7B Instruct v0.2 Q5_K_M",
        UrlEnv = "LOCAL_LLM_7B_DOWNLOAD_URL",
        DefaultUrl = "https://huggingface.co/TheBloke/Mistral-7B-Instruct-v0.2-GGUF/resolve/main/mistral-7b-instruct-v0.2.Q5_K_M.gguf",
        RelativePath = "mistral-7b-instruct-v0.2.Q5_K_M.gguf",
        Kind = GgufArtifactKind.TextModel,
        Functions = [GgufFunction.Categorization, GgufFunction.Documents],
        Notes = "Larger local categorization and document model."
    };

    public static readonly GgufArtifact Gemma11SevenB = new()
    {
        Id = "gemma-1.1-7b",
        DisplayName = "Gemma 1.1 7B IT Q5_K_M",
        UrlEnv = "LOCAL_LLM_7B_GEMMA_DOWNLOAD_URL",
        DefaultUrl = "https://huggingface.co/bartowski/gemma-1.1-7b-it-GGUF/resolve/main/gemma-1.1-7b-it-Q5_K_M.gguf",
        RelativePath = "gemma-1.1-7b-it-Q5_K_M.gguf",
        Kind = GgufArtifactKind.TextModel,
        Functions = [GgufFunction.Categorization, GgufFunction.Documents]
    };

    public static readonly GgufArtifact Llama3BLegacy = new()
    {
        Id = "llama-3b-legacy",
        DisplayName = "LLaMa 3b v3.2 Instruct Q8, legacy",
        UrlEnv = "LOCAL_LLM_3B_LEGACY_DOWNLOAD_URL",
        DefaultUrl = "https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q8_0.gguf",
        RelativePath = "Llama-3.2-3B-Instruct-Q8_0.gguf",
        Kind = GgufArtifactKind.TextModel,
        Functions = [GgufFunction.Categorization, GgufFunction.Documents],
        Notes = "Legacy local GGUF kept for existing downloads."
    };

    public static readonly GgufArtifact Gemma3VisualModel = new()
    {
        Id = "gemma-3-4b-it-visual-model",
        DisplayName = "Gemma 3 4B IT (text model)",
        UrlEnv = "GEMMA3_4B_MODEL_URL",
        DefaultUrl = "https://huggingface.co/ggml-org/gemma-3-4b-it-GGUF/resolve/main/gemma-3-4b-it-Q4_K_M.gguf",
        RelativePath = Path.Combine("gemma-3-4b-it", "model.gguf"),
        Kind = GgufArtifactKind.TextModel,
        Functions = [GgufFunction.ImageAnalysis],
        Notes = "Visual backend text weights. Same GGUF as the default categorization model.",
        SharesUrlWithArtifactId = "gemma-3-4b-it"
    };

    public static readonly GgufArtifact Gemma3Mmproj = new()
    {
        Id = "gemma-3-4b-it-mmproj",
        DisplayName = "Gemma 3 4B mmproj (vision encoder)",
        UrlEnv = "GEMMA3_4B_MMPROJ_URL",
        DefaultUrl = "https://huggingface.co/ggml-org/gemma-3-4b-it-GGUF/resolve/main/mmproj-model-f16.gguf",
        RelativePath = Path.Combine("gemma-3-4b-it", "mmproj.gguf"),
        Kind = GgufArtifactKind.Mmproj,
        Functions = [GgufFunction.ImageAnalysis]
    };

    public static readonly GgufArtifact LlavaModel = new()
    {
        Id = "llava-v1.6-mistral-7b-model",
        DisplayName = "LLaVA 1.6 Mistral 7B (text model)",
        UrlEnv = "LLAVA_MODEL_URL",
        DefaultUrl = "https://huggingface.co/cjpais/llava-1.6-mistral-7b-gguf/resolve/main/llava-v1.6-mistral-7b.Q4_K_M.gguf",
        RelativePath = Path.Combine("llava-v1.6-mistral-7b", "model.gguf"),
        Kind = GgufArtifactKind.TextModel,
        Functions = [GgufFunction.ImageAnalysis]
    };

    public static readonly GgufArtifact LlavaMmproj = new()
    {
        Id = "llava-v1.6-mistral-7b-mmproj",
        DisplayName = "LLaVA mmproj (vision encoder)",
        UrlEnv = "LLAVA_MMPROJ_URL",
        DefaultUrl = "https://huggingface.co/cjpais/llava-1.6-mistral-7b-gguf/resolve/main/mmproj-model-f16.gguf",
        RelativePath = Path.Combine("llava-v1.6-mistral-7b", "mmproj.gguf"),
        Kind = GgufArtifactKind.Mmproj,
        Functions = [GgufFunction.ImageAnalysis]
    };

    public static IReadOnlyList<GgufModelChoice> CategorizationModels { get; } =
    [
        new() { Id = Gemma3Categorization.Id, DisplayName = Gemma3Categorization.DisplayName, Artifact = Gemma3Categorization },
        new() { Id = Mistral7B.Id, DisplayName = Mistral7B.DisplayName, Artifact = Mistral7B },
        new() { Id = Gemma11SevenB.Id, DisplayName = Gemma11SevenB.DisplayName, Artifact = Gemma11SevenB },
        new() { Id = Llama3BLegacy.Id, DisplayName = Llama3BLegacy.DisplayName, Artifact = Llama3BLegacy, IsLegacy = true }
    ];

    public static IReadOnlyList<GgufVisualBackend> VisualBackends { get; } =
    [
        new() { Id = "gemma-3-4b-it", DisplayName = "Gemma 3 4B IT", TextModel = Gemma3VisualModel, Mmproj = Gemma3Mmproj },
        new() { Id = "llava-v1.6-mistral-7b", DisplayName = "LLaVA 1.6 Mistral 7B", TextModel = LlavaModel, Mmproj = LlavaMmproj }
    ];

    public static IReadOnlyList<GgufArtifact> AllArtifacts { get; } =
    [
        Gemma3Categorization, Mistral7B, Gemma11SevenB, Llama3BLegacy,
        Gemma3VisualModel, Gemma3Mmproj, LlavaModel, LlavaMmproj
    ];

    public static GgufArtifact? FindArtifact(string id) =>
        AllArtifacts.FirstOrDefault(artifact => artifact.Id.Equals(id, StringComparison.OrdinalIgnoreCase));

    public static GgufModelChoice? FindCategorizationModel(string id) =>
        CategorizationModels.FirstOrDefault(model => model.Id.Equals(id, StringComparison.OrdinalIgnoreCase));

    public static GgufVisualBackend? FindVisualBackend(string id) =>
        VisualBackends.FirstOrDefault(backend => backend.Id.Equals(id, StringComparison.OrdinalIgnoreCase));

    public static string FormatFunctions(GgufArtifact artifact) =>
        string.Join(", ", artifact.Functions.Select(function => function switch
        {
            GgufFunction.Categorization => "Categorization",
            GgufFunction.Documents => "Documents",
            GgufFunction.ImageAnalysis => artifact.Kind == GgufArtifactKind.Mmproj
                ? "Image analysis (mmproj)"
                : "Image analysis (text model)",
            _ => function.ToString()
        }));

    public static string ResolveCategorizationModelPath(LlmEndpointSettings settings)
    {
        if (!string.IsNullOrWhiteSpace(settings.LocalGgufPath) && File.Exists(settings.LocalGgufPath))
        {
            return settings.LocalGgufPath;
        }

        var choice = FindCategorizationModel(settings.BuiltinLocalModelId) ?? CategorizationModels[0];
        return GgufStorage.ResolveExistingPath(choice.Artifact, settings.ModelStorageDir) ?? "";
    }

    public static (string? ModelPath, string? MmprojPath) ResolveVisualPaths(LlmEndpointSettings settings)
    {
        if (!string.IsNullOrWhiteSpace(settings.LocalGgufPath) &&
            !string.IsNullOrWhiteSpace(settings.LocalMmprojPath) &&
            File.Exists(settings.LocalGgufPath) &&
            File.Exists(settings.LocalMmprojPath))
        {
            return (settings.LocalGgufPath, settings.LocalMmprojPath);
        }

        var backend = FindVisualBackend(settings.VisualBackendId) ?? VisualBackends[0];
        return (
            GgufStorage.ResolveExistingPath(backend.TextModel, settings.ModelStorageDir),
            GgufStorage.ResolveExistingPath(backend.Mmproj, settings.ModelStorageDir));
    }
}
