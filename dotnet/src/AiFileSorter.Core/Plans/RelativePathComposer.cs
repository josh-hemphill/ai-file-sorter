using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Plans;

/// <summary>Builds a reviewable destination path from category, subcategory, and rename suggestions.</summary>
public static class RelativePathComposer
{
    public static CategorizedItem WithLocalPath(
        CategorizedItem item,
        string rootPath,
        ContentAnalysisOptions content)
    {
        var relative = Compose(item, rootPath, content);
        return item with
        {
            LocalRelativePath = relative,
            AcceptedRelativePath = item.AcceptedRelativePath,
            Status = item.Status == default ? SuggestionStatus.Local : item.Status
        };
    }

    public static string Compose(CategorizedItem item, string rootPath, ContentAnalysisOptions content)
    {
        var fileName = item.SuggestedName ?? item.FileName;
        if (item.Kind == EntryKind.Directory && !item.IsArchiveEntity)
        {
            fileName = item.FileName;
        }

        fileName = SanitizeSegment(fileName);
        if (item.RenameOnly(content))
        {
            var current = CurrentRelativeDirectory(item.FullPath, rootPath);
            return Combine(current, fileName);
        }

        var category = SanitizeSegment(item.Category);
        if (!content.UseSubcategories || string.IsNullOrWhiteSpace(item.Subcategory))
        {
            return Combine(category, fileName);
        }

        return Combine(category, SanitizeSegment(item.Subcategory), fileName);
    }

    public static string SanitizeRelativePath(string relative)
    {
        var parts = relative.Replace('\\', '/').Split('/', StringSplitOptions.RemoveEmptyEntries);
        if (parts.Any(part => part == "." || part == ".."))
        {
            throw new InvalidOperationException("Proposed path must stay inside the analyzed folder.");
        }

        return string.Join('/', parts.Select(SanitizeSegment));
    }

    public static string Combine(params string[] parts) =>
        string.Join('/', parts.Where(part => !string.IsNullOrWhiteSpace(part)).Select(SanitizeSegment));

    public static string SanitizeSegment(string value)
    {
        var invalid = Path.GetInvalidFileNameChars();
        var chars = value.Trim().ToCharArray();
        for (var i = 0; i < chars.Length; i++)
        {
            if (invalid.Contains(chars[i]) || chars[i] is '/' or '\\')
            {
                chars[i] = '_';
            }
        }

        var cleaned = new string(chars).Trim();
        return string.IsNullOrWhiteSpace(cleaned) ? "untitled" : cleaned;
    }

    private static string CurrentRelativeDirectory(string fullPath, string rootPath)
    {
        var full = Path.GetFullPath(fullPath);
        var root = Path.GetFullPath(rootPath);
        var relative = Path.GetRelativePath(root, full);
        var directory = Path.GetDirectoryName(relative)?.Replace('\\', '/') ?? "";
        return directory is "." ? "" : directory;
    }
}

internal static class CategorizedItemContentExtensions
{
    public static bool RenameOnly(this CategorizedItem item, ContentAnalysisOptions content)
    {
        if (item.Family == FileFamily.Document)
        {
            return content.RenameDocumentsOnly;
        }

        if (item.Family == FileFamily.Image)
        {
            return content.RenameImagesOnly;
        }

        return false;
    }
}
