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
    [
        new() { Id = "gemma-3-4b-it", Kind = LlmKind.LocalGguf, DisplayName = "Gemma 3 4B IT Q4_K_M", Notes = "Default local GGUF. Inference stays in the C++ sidecar." },
        new() { Id = "mistral-7b", Kind = LlmKind.LocalGguf, DisplayName = "Mistral 7B Instruct v0.2 Q5_K_M", Notes = "Local GGUF sidecar." },
        new() { Id = "gemma-1.1-7b", Kind = LlmKind.LocalGguf, DisplayName = "Gemma 1.1 7B IT Q5_K_M", Notes = "Local GGUF sidecar." },
        new() { Id = "llama-3b-legacy", Kind = LlmKind.LocalGguf, DisplayName = "LLaMa 3b v3.2 Instruct Q8, legacy", Notes = "Legacy local GGUF sidecar." }
    ];

    public static IReadOnlyList<LlmCatalogEntry> VisualBackends { get; } =
    [
        new() { Id = "gemma-3-4b-it", Kind = LlmKind.LocalGguf, DisplayName = "Gemma 3 4B IT", IsVisual = true, Notes = "Visual sidecar for picture content. Not compiled into the AOT UI." },
        new() { Id = "llava-v1.6-mistral-7b", Kind = LlmKind.LocalGguf, DisplayName = "LLaVA 1.6 Mistral 7B", IsVisual = true, Notes = "Visual sidecar for picture content." }
    ];

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
        LlmKind.LocalGguf => BuiltinLocalModels.FirstOrDefault(entry => entry.Id == settings.BuiltinLocalModelId)?.DisplayName
            ?? "Local GGUF sidecar",
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
}
