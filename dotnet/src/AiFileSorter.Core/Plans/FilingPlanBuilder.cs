using AiFileSorter.Core.Categorization;
using AiFileSorter.Core.IO;
using AiFileSorter.Core.Media;
using AiFileSorter.Core.Models;
using AiFileSorter.Core.Scanning;

namespace AiFileSorter.Core.Plans;

/// <summary>Turns a scan into a reviewable filing plan, skipping files locked by other processes.</summary>
public sealed class FilingPlanBuilder
{
    private readonly MediaMetadataReader _mediaReader;
    private readonly MediaContentCategorizer _mediaCategorizer;

    public FilingPlanBuilder()
        : this(new MediaMetadataReader(), new MediaContentCategorizer())
    {
    }

    public FilingPlanBuilder(MediaMetadataReader mediaReader, MediaContentCategorizer mediaCategorizer)
    {
        _mediaReader = mediaReader;
        _mediaCategorizer = mediaCategorizer;
    }

    public FilingPlan Build(
        string rootPath,
        ScanResult scan,
        AnalysisRequest request,
        IProgress<AnalysisProgress>? progress,
        CancellationToken cancellationToken)
    {
        var entries = new List<CategorizedItem>(scan.Items.Count);
        var locked = new List<CategorizedItem>();
        var current = 0;

        foreach (var item in scan.Items)
        {
            cancellationToken.ThrowIfCancellationRequested();
            current++;
            progress?.Report(new AnalysisProgress
            {
                Stage = "categorize",
                Current = current,
                Total = scan.Items.Count,
                Message = item.FileName
            });

            var categorized = Categorize(item, request);
            if (categorized.IsLocked)
            {
                locked.Add(categorized);
            }

            entries.Add(categorized);
        }

        foreach (var archive in scan.ArchiveEntities)
        {
            if (entries.Any(entry => PathsEqual(entry.FullPath, archive.RootPath)))
            {
                continue;
            }

            entries.Add(new CategorizedItem
            {
                FullPath = archive.RootPath,
                FileName = Path.GetFileName(archive.RootPath),
                Kind = EntryKind.Directory,
                Family = FileFamily.Archive,
                Category = "Archives",
                Subcategory = "Projects",
                SuggestedName = archive.SuggestedArchiveName,
                Rationale = archive.Reason,
                IsArchiveEntity = true,
                ArchiveFormat = archive.Format
            });
        }

        var files = entries.Count(entry => entry.Kind == EntryKind.File);
        var directories = entries.Count(entry => entry.Kind == EntryKind.Directory);
        return new FilingPlan
        {
            RootPath = Path.GetFullPath(rootPath),
            Style = request.Style,
            Entries = entries,
            ArchiveEntities = scan.ArchiveEntities,
            LockedItems = locked,
            Summary = new FilingPlanSummary
            {
                FileCount = files,
                DirectoryCount = directories,
                ArchiveEntityCount = scan.ArchiveEntities.Count,
                LockedCount = locked.Count,
                ByFamily = CountBy(entries, entry => entry.Family.ToString()),
                ByCategory = CountBy(entries, entry => entry.Category)
            }
        };
    }

    private CategorizedItem Categorize(ScannedItem item, AnalysisRequest request)
    {
        if (item.Kind == EntryKind.Directory && item.Family == FileFamily.Archive)
        {
            var archive = HeuristicCategorizer.Categorize(item, request.Style) with
            {
                IsArchiveEntity = true,
                ArchiveFormat = ArchiveFormat.Zip,
                SuggestedName = Path.GetFileName(item.FullPath) + ".zip"
            };
            return archive;
        }

        if (item.Kind == EntryKind.File && request.AnalyzeMediaContent && FileFamilyClassifier.IsMedia(item.FileName))
        {
            string? lockReason = null;
            using var stream = LockTolerantFileAccess.TryOpenRead(item.FullPath, out lockReason);
            if (stream is null)
            {
                var locked = HeuristicCategorizer.Categorize(item, request.Style) with
                {
                    IsLocked = true,
                    LockReason = lockReason,
                    Rationale = "Skipped media content analysis because the file is locked by another process."
                };
                return locked;
            }

            stream.Dispose();
            var metadata = _mediaReader.Read(item.FullPath);
            if (metadata?.Source?.StartsWith("locked:", StringComparison.Ordinal) == true)
            {
                return HeuristicCategorizer.Categorize(item, request.Style) with
                {
                    IsLocked = true,
                    LockReason = metadata.Source["locked:".Length..],
                    Rationale = "Skipped media content analysis because the file is locked by another process."
                };
            }

            return _mediaCategorizer.Categorize(item, metadata, request.SuggestMediaRenames, request.Style);
        }

        return HeuristicCategorizer.Categorize(item, request.Style);
    }

    private static Dictionary<string, int> CountBy(IEnumerable<CategorizedItem> entries, Func<CategorizedItem, string> keySelector)
    {
        var counts = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        foreach (var entry in entries)
        {
            var key = keySelector(entry);
            counts[key] = counts.TryGetValue(key, out var count) ? count + 1 : 1;
        }

        return counts;
    }

    private static bool PathsEqual(string left, string right) =>
        string.Equals(Path.GetFullPath(left), Path.GetFullPath(right), StringComparison.OrdinalIgnoreCase);
}
