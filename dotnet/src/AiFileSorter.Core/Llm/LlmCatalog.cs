using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Llm;

public sealed record LlmCatalogEntry
{
    public required string Id { get; init; }
    public required LlmKind Kind { get; init; }
    public required string DisplayName { get; init; }
    public string Notes { get; init; } = "";
    public bool IsVisual { get; init; }
}

/// <summary>Built-in model labels matching the Qt LLM selection dialog.</summary>
public static class LlmCatalog
{
    public static IReadOnlyList<LlmCatalogEntry> RemoteEndpoints { get; } =
    [
        new() { Id = "heuristic", Kind = LlmKind.Heuristic, DisplayName = "Heuristic (local metadata)", Notes = "No network. Uses file family, tags, and filename cues." },
        new() { Id = "openai", Kind = LlmKind.OpenAi, DisplayName = "OpenAI", Notes = "Remote chat completions. The engine sends a compact filing plan, not the files themselves." },
        new() { Id = "gemini", Kind = LlmKind.Gemini, DisplayName = "Gemini", Notes = "Remote Gemini generateContent. Used for path proposals, not on-disk moves." },
        new() { Id = "custom-api", Kind = LlmKind.CustomApi, DisplayName = "Custom OpenAI-compatible API", Notes = "Any OpenAI-compatible /chat/completions endpoint." }
    ];

    public static IReadOnlyList<LlmCatalogEntry> BuiltinLocalModels { get; } =
        GgufCatalog.CategorizationModels.Select(model => new LlmCatalogEntry
        {
            Id = model.Id,
            Kind = LlmKind.LocalGguf,
            DisplayName = model.Artifact.ResolveDisplayName(),
            Notes = GgufCatalog.FormatFunctions(model.Artifact)
        }).ToArray();

    public static IReadOnlyList<LlmCatalogEntry> VisualBackends { get; } =
        GgufCatalog.VisualBackends.Select(backend => new LlmCatalogEntry
        {
            Id = backend.Id,
            Kind = LlmKind.LocalGguf,
            DisplayName = backend.DisplayName,
            IsVisual = true,
            Notes = "Requires text GGUF + mmproj for picture analysis."
        }).ToArray();

    public static IReadOnlyList<string> WhitelistNames { get; } =
        ["Unrestricted", "Media library", "Documents"];

    public static IReadOnlySet<string>? AllowedCategories(string whitelistName) => whitelistName switch
    {
        "Media library" => new HashSet<string>(StringComparer.OrdinalIgnoreCase)
        {
            "Audio", "Videos", "Images", "Archives", "Folders"
        },
        "Documents" => new HashSet<string>(StringComparer.OrdinalIgnoreCase)
        {
            "Documents", "Presentations", "Spreadsheets", "Data Exports", "Configs", "Ebooks", "Folders"
        },
        _ => null
    };

    public static string Summarize(LlmEndpointSettings settings) => settings.Kind switch
    {
        LlmKind.OpenAi => $"OpenAI ({settings.OpenAiModel})",
        LlmKind.Gemini => $"Gemini ({settings.GeminiModel})",
        LlmKind.CustomApi => string.IsNullOrWhiteSpace(settings.CustomName)
            ? "Custom API"
            : settings.CustomName,
        LlmKind.LocalGguf => DescribeLocal(settings),
        _ => "Heuristic (local metadata)"
    };

    public static (string EndpointUrl, string Model, string? ApiKey)? ResolveRemoteEndpoint(LlmEndpointSettings settings)
    {
        return settings.Kind switch
        {
            LlmKind.OpenAi when !string.IsNullOrWhiteSpace(settings.OpenAiApiKey) =>
                ("https://api.openai.com/v1/chat/completions", settings.OpenAiModel, settings.OpenAiApiKey),
            LlmKind.CustomApi when !string.IsNullOrWhiteSpace(settings.CustomBaseUrl) =>
                (settings.CustomBaseUrl, settings.CustomModel, string.IsNullOrWhiteSpace(settings.CustomApiKey) ? null : settings.CustomApiKey),
            LlmKind.Gemini when !string.IsNullOrWhiteSpace(settings.GeminiApiKey) =>
                (
                    $"https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
                    settings.GeminiModel,
                    settings.GeminiApiKey
                ),
            _ => null
        };
    }

    private static string DescribeLocal(LlmEndpointSettings settings)
    {
        var choice = GgufCatalog.FindCategorizationModel(settings.BuiltinLocalModelId);
        var path = GgufCatalog.ResolveCategorizationModelPath(settings);
        var name = choice?.Artifact.ResolveDisplayName() ?? "Local GGUF";
        return string.IsNullOrWhiteSpace(path) ? $"{name} (not downloaded)" : $"{name} (ready)";
    }
}
