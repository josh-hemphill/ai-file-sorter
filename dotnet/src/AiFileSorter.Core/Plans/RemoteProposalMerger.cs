using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Plans;

/// <summary>Merges remote per-file path proposals into the local suggestion database without touching disk.</summary>
public static class RemoteProposalMerger
{
    public static FilingPlan Merge(FilingPlan plan, RemoteStructureSuggestion proposal)
    {
        if (proposal.Updates.Count == 0)
        {
            return MergeFolderHints(plan, proposal);
        }

        var byFullPath = new Dictionary<string, RemotePathUpdate>(StringComparer.OrdinalIgnoreCase);
        var byFileName = new Dictionary<string, RemotePathUpdate>(StringComparer.OrdinalIgnoreCase);
        foreach (var update in proposal.Updates)
        {
            if (!string.IsNullOrWhiteSpace(update.FullPath))
            {
                byFullPath[update.FullPath] = update;
            }

            if (!string.IsNullOrWhiteSpace(update.FileName) && !byFileName.ContainsKey(update.FileName))
            {
                byFileName[update.FileName] = update;
            }
        }

        var entries = plan.Entries.Select(entry =>
        {
            if (!byFullPath.TryGetValue(entry.FullPath, out var update) &&
                !byFileName.TryGetValue(entry.FileName, out update))
            {
                return entry;
            }

            var remotePath = RelativePathComposer.SanitizeRelativePath(update.ProposedRelativePath);
            var conflict = !string.Equals(entry.LocalRelativePath, remotePath, StringComparison.OrdinalIgnoreCase)
                && entry.Status is SuggestionStatus.Accepted or SuggestionStatus.Applied;
            return entry with
            {
                RemoteRelativePath = remotePath,
                Category = string.IsNullOrWhiteSpace(update.Category) ? entry.Category : update.Category,
                Subcategory = update.Subcategory ?? entry.Subcategory,
                Rationale = string.IsNullOrWhiteSpace(update.Rationale)
                    ? entry.Rationale
                    : update.Rationale,
                Status = conflict ? SuggestionStatus.Conflict : SuggestionStatus.RemoteProposed
            };
        }).ToArray();

        return plan with { Entries = entries };
    }

    private static FilingPlan MergeFolderHints(FilingPlan plan, RemoteStructureSuggestion proposal)
    {
        if (proposal.Folders.Count == 0)
        {
            return plan;
        }

        var folders = proposal.Folders
            .GroupBy(folder => folder.Category, StringComparer.OrdinalIgnoreCase)
            .ToDictionary(group => group.Key, group => group.First(), StringComparer.OrdinalIgnoreCase);

        var entries = plan.Entries.Select(entry =>
        {
            if (!folders.TryGetValue(entry.Category, out var folder))
            {
                return entry;
            }

            var updated = entry with
            {
                Subcategory = folder.Subcategory ?? entry.Subcategory,
                Rationale = string.IsNullOrWhiteSpace(folder.Notes) ? entry.Rationale : folder.Notes,
                Status = SuggestionStatus.RemoteProposed
            };
            var remotePath = RelativePathComposer.Compose(updated, plan.RootPath, new ContentAnalysisOptions());
            return updated with { RemoteRelativePath = remotePath };
        }).ToArray();

        return plan with { Entries = entries };
    }
}
