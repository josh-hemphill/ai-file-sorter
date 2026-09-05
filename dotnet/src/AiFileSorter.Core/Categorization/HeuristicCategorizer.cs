using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Categorization;

/// <summary>Builds a local category when no media-specific or remote model result is available.</summary>
public static class HeuristicCategorizer
{
    public static CategorizedItem Categorize(ScannedItem item, CategorizationStyle style)
    {
        if (item.Kind == EntryKind.Directory)
        {
            return new CategorizedItem
            {
                FullPath = item.FullPath,
                FileName = item.FileName,
                Kind = item.Kind,
                Family = item.Family,
                Category = item.Family == FileFamily.Archive ? "Archives" : "Folders",
                Subcategory = item.Family == FileFamily.Archive ? "Projects" : null,
                Rationale = item.LockReason ?? "Directory kept as a folder-level suggestion.",
                IsLocked = item.IsLocked,
                LockReason = item.LockReason
            };
        }

        var category = FileFamilyClassifier.PreferredCategory(item.FileName, item.Family, style);
        var subcategory = item.Family switch
        {
            FileFamily.Software => SuggestSoftwareSubcategory(item.FileName),
            FileFamily.Image => "Photos",
            FileFamily.Document => null,
            _ => null
        };

        return new CategorizedItem
        {
            FullPath = item.FullPath,
            FileName = item.FileName,
            Kind = item.Kind,
            Family = item.Family,
            Category = category,
            Subcategory = subcategory,
            Rationale = $"Classified from file family '{item.Family}'.",
            IsLocked = item.IsLocked,
            LockReason = item.LockReason
        };
    }

    private static string SuggestSoftwareSubcategory(string fileName)
    {
        var lower = fileName.ToLowerInvariant();
        if (lower.EndsWith(".msi", StringComparison.Ordinal) ||
            lower.EndsWith(".exe", StringComparison.Ordinal) ||
            lower.EndsWith(".dmg", StringComparison.Ordinal) ||
            lower.EndsWith(".pkg", StringComparison.Ordinal))
        {
            return "Installers";
        }

        return "Binaries";
    }
}
