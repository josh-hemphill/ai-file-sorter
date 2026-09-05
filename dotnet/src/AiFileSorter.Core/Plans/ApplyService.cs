using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Plans;

/// <summary>Applies accepted suggestion rows locally. Remote models never move files.</summary>
public sealed class ApplyService
{
    public ApplyDryRun Preview(FilingPlan plan)
    {
        var moves = new List<ApplyMove>();
        var warnings = new List<string>();
        var destinations = new HashSet<string>(StringComparer.OrdinalIgnoreCase);

        foreach (var entry in plan.Entries)
        {
            if (!entry.Selected || entry.Status is SuggestionStatus.Rejected or SuggestionStatus.Applied)
            {
                continue;
            }

            if (entry.IsArchiveEntity)
            {
                warnings.Add($"{entry.FileName}: archive-entity suggestions are not auto-zipped yet.");
                continue;
            }

            if (entry.Status is SuggestionStatus.RemoteProposed or SuggestionStatus.Conflict)
            {
                warnings.Add($"{entry.FileName}: remote proposal is pending review (accept or keep local first).");
                continue;
            }

            if (entry.Status is not (SuggestionStatus.Accepted or SuggestionStatus.Local))
            {
                continue;
            }

            string relative;
            try
            {
                relative = RelativePathComposer.SanitizeRelativePath(entry.EffectiveRelativePath);
            }
            catch (InvalidOperationException ex)
            {
                warnings.Add($"{entry.FileName}: {ex.Message}");
                continue;
            }

            var destination = Path.GetFullPath(Path.Combine(plan.RootPath, relative));
            var source = Path.GetFullPath(entry.FullPath);
            if (string.Equals(source, destination, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            if (!File.Exists(source) && !Directory.Exists(source))
            {
                warnings.Add($"{entry.FileName}: source is missing.");
                continue;
            }

            if (!destinations.Add(destination))
            {
                warnings.Add($"{entry.FileName}: destination '{relative}' collides with another selected row.");
                continue;
            }

            moves.Add(new ApplyMove(source, destination, relative, entry.FileName));
        }

        return new ApplyDryRun { Moves = moves, Warnings = warnings };
    }

    public ApplyResult Apply(FilingPlan plan, bool dryRun)
    {
        var preview = Preview(plan);
        if (dryRun)
        {
            return new ApplyResult { DryRun = true, Applied = preview.Moves, Errors = preview.Warnings };
        }

        var applied = new List<ApplyMove>();
        var errors = new List<string>(preview.Warnings);
        foreach (var move in preview.Moves)
        {
            try
            {
                var destinationDirectory = Path.GetDirectoryName(move.DestinationPath);
                if (!string.IsNullOrWhiteSpace(destinationDirectory))
                {
                    Directory.CreateDirectory(destinationDirectory);
                }

                if (Directory.Exists(move.SourcePath))
                {
                    Directory.Move(move.SourcePath, move.DestinationPath);
                }
                else
                {
                    File.Move(move.SourcePath, move.DestinationPath);
                }

                applied.Add(move);
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                errors.Add($"{move.FileName}: {ex.Message}");
            }
        }

        string? undoPath = null;
        if (applied.Count > 0)
        {
            undoPath = Path.Combine(plan.RootPath, "aifs-undo-plan.json");
            var undo = new ApplyDryRun
            {
                Moves = applied.Select(move => new ApplyMove(
                    move.DestinationPath,
                    move.SourcePath,
                    Path.GetRelativePath(plan.RootPath, move.SourcePath).Replace('\\', '/'),
                    move.FileName)).ToArray()
            };
            File.WriteAllText(undoPath, Json.AppJson.Serialize(undo));
        }

        return new ApplyResult
        {
            DryRun = false,
            Applied = applied,
            Errors = errors,
            UndoPlanPath = undoPath
        };
    }

    public static FilingPlan MarkApplied(FilingPlan plan, ApplyResult result)
    {
        if (result.DryRun || result.Applied.Count == 0)
        {
            return plan;
        }

        var appliedSources = result.Applied.Select(move => move.SourcePath).ToHashSet(StringComparer.OrdinalIgnoreCase);
        var entries = plan.Entries.Select(entry =>
        {
            if (!appliedSources.Contains(Path.GetFullPath(entry.FullPath)))
            {
                return entry;
            }

            var move = result.Applied.First(item => string.Equals(item.SourcePath, Path.GetFullPath(entry.FullPath), StringComparison.OrdinalIgnoreCase));
            return entry with
            {
                FullPath = move.DestinationPath,
                FileName = Path.GetFileName(move.DestinationPath),
                Status = SuggestionStatus.Applied,
                AcceptedRelativePath = move.RelativePath
            };
        }).ToArray();

        return plan with { Entries = entries };
    }
}
