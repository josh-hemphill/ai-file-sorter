using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Llm;

public static class LocalLlmPrompts
{
    public static string CategorizationSystem(CategorizationStyle style) =>
        "You are a file categorization assistant. If the file is an installer, determine the type of software it installs. " +
        "Base your answer on the filename, extension, and any directory context provided. " +
        (style == CategorizationStyle.Consistent
            ? "Prefer stable, filesystem-friendly main categories. "
            : "Prefer a semantically accurate main category. ") +
        "Reply with exactly one line in the format <Main category> : <Subcategory>. " +
        "Main category must be broad (one or two words, plural). Subcategory must be specific and must not repeat the main category. " +
        "Do not explain your answer.";

    public static string DocumentSystem(CategorizationStyle style) =>
        "You are a document categorization assistant. Categorize the document by its subject matter and content, " +
        "not merely by its file extension. Use any provided document summary as the primary evidence. " +
        (style == CategorizationStyle.Consistent
            ? "Keep the main category broad and filesystem-friendly. "
            : "Choose the most semantically accurate main category. ") +
        "Reply with exactly one line in the format <Main category> : <Subcategory>. Do not explain your answer.";

    public static string ImageSystem(CategorizationStyle style) =>
        "You categorize image files for filesystem organization. " +
        (style == CategorizationStyle.Consistent
            ? "Always use Images as the main category. "
            : "You may use Images as the main category, or a more content-specific main category when that clearly improves organization. ") +
        "Use any provided image description as the primary evidence. " +
        "Reply with exactly one line in the format <Main category> : <Subcategory>. Do not explain your answer.";

    public static string CategorizationUser(string fileName, string fullPath, EntryKind kind) =>
        $"{(kind == EntryKind.File ? "Categorize this file." : "Categorize this directory.")}\n" +
        $"Full path: {fullPath}\n" +
        $"{(kind == EntryKind.File ? "File name" : "Directory name")}: {fileName}\n\n" +
        "Answer with exactly one line:\n<Main category> : <Subcategory>";

    public static string DocumentUser(string fileName, string fullPath, string? summary)
    {
        var prompt = $"Categorize this document for file organization.\nFile name: {fileName}\nPath: {fullPath}\n";
        if (!string.IsNullOrWhiteSpace(summary))
        {
            prompt += "Document summary: " + summary + "\n";
        }

        return prompt + "\nAnswer with exactly one line:\n<Main category> : <Subcategory>";
    }

    public static string ImageUser(string fileName, string fullPath, string? description)
    {
        var prompt = $"Categorize this image file for file organization.\nFile name: {fileName}\nPath: {fullPath}\n";
        if (!string.IsNullOrWhiteSpace(description))
        {
            prompt += "Image description: " + description + "\n";
        }

        return prompt + "\nAnswer with exactly one line:\n<Main category> : <Subcategory>";
    }

    public static bool TryParseCategoryLine(string response, out string category, out string? subcategory)
    {
        category = "";
        subcategory = null;
        if (string.IsNullOrWhiteSpace(response))
        {
            return false;
        }

        var line = response.Replace('\r', '\n').Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .FirstOrDefault(part => part.Contains(':'));
        if (string.IsNullOrWhiteSpace(line))
        {
            return false;
        }

        var separator = line.IndexOf(':');
        category = line[..separator].Trim().Trim('<', '>');
        var rest = line[(separator + 1)..].Trim().Trim('<', '>');
        subcategory = string.IsNullOrWhiteSpace(rest) ? null : rest;
        return category.Length > 0 &&
               !category.Equals("Category", StringComparison.OrdinalIgnoreCase) &&
               !category.Equals("Main category", StringComparison.OrdinalIgnoreCase);
    }
}
