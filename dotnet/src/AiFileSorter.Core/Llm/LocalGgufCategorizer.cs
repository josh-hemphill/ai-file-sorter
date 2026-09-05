using AiFileSorter.Core.IO;
using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Llm;

/// <summary>Applies a local GGUF (via sidecar) to categorization, document, and image-filename prompts.</summary>
public sealed class LocalGgufCategorizer
{
    private readonly ILocalLlmClient _client;
    private readonly string _modelPath;
    private readonly string? _mmprojPath;

    public LocalGgufCategorizer(ILocalLlmClient client, string modelPath, string? mmprojPath = null)
    {
        _client = client;
        _modelPath = modelPath;
        _mmprojPath = mmprojPath;
    }

    public CategorizedItem? TryCategorize(
        ScannedItem item,
        ContentAnalysisOptions content,
        CategorizationStyle style,
        CategorizedItem fallback)
    {
        try
        {
            string system;
            string user;
            if (item.Family == FileFamily.Document && content.AnalyzeDocuments)
            {
                system = LocalLlmPrompts.DocumentSystem(style);
                user = LocalLlmPrompts.DocumentUser(item.FileName, item.FullPath, ReadTextPreview(item.FullPath));
            }
            else if (item.Family == FileFamily.Image && content.AnalyzeImages)
            {
                system = LocalLlmPrompts.ImageSystem(style);
                user = LocalLlmPrompts.ImageUser(item.FileName, item.FullPath, description: null);
            }
            else
            {
                system = LocalLlmPrompts.CategorizationSystem(style);
                user = LocalLlmPrompts.CategorizationUser(item.FileName, item.FullPath, item.Kind);
            }

            var text = _client.Complete(new LlamaCompleteRequest
            {
                ModelPath = _modelPath,
                MmprojPath = item.Family == FileFamily.Image ? _mmprojPath : null,
                SystemPrompt = system,
                UserPrompt = user,
                MaxTokens = 64
            });

            if (!LocalLlmPrompts.TryParseCategoryLine(text, out var category, out var subcategory))
            {
                return fallback with { Rationale = "Local GGUF returned an unparsable category line; kept the heuristic result. Raw: " + TrimRaw(text) };
            }

            return fallback with
            {
                Category = category,
                Subcategory = subcategory,
                Rationale = item.Family == FileFamily.Document && content.AnalyzeDocuments
                    ? "Categorized by the local GGUF using a document excerpt."
                    : item.Family == FileFamily.Image && content.AnalyzeImages
                        ? "Categorized by the local GGUF image prompt."
                        : "Categorized by the local GGUF sidecar."
            };
        }
        catch (Exception ex) when (ex is InvalidOperationException or FileNotFoundException or TimeoutException or IOException)
        {
            return fallback with { Rationale = "Local GGUF sidecar failed (" + ex.Message + "); kept the heuristic result." };
        }
    }

    private static string? ReadTextPreview(string path)
    {
        string? lockReason;
        using var stream = LockTolerantFileAccess.TryOpenRead(path, out lockReason);
        if (stream is null)
        {
            return null;
        }

        using var reader = new StreamReader(stream);
        var buffer = new char[4000];
        var read = reader.Read(buffer, 0, buffer.Length);
        if (read <= 0)
        {
            return null;
        }

        return new string(buffer, 0, read);
    }

    private static string TrimRaw(string text)
    {
        var trimmed = text.Trim();
        return trimmed.Length <= 160 ? trimmed : trimmed[..160] + "…";
    }
}
