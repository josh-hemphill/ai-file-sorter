using AiFileSorter.Core.Categorization;
using AiFileSorter.Core.IO;
using AiFileSorter.Core.Llm;
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
        var content = request.Content;
        var allowed = content.UseWhitelist ? LlmCatalog.AllowedCategories(content.ActiveWhitelist) : null;
        var entries = new List<CategorizedItem>(scan.Items.Count);
        var locked = new List<CategorizedItem>();
        var current = 0;
        var candidates = scan.Items.Where(item => ShouldInclude(item, content)).ToArray();

        foreach (var item in candidates)
        {
            cancellationToken.ThrowIfCancellationRequested();
            current++;
            progress?.Report(new AnalysisProgress
            {
                Stage = "categorize",
                Current = current,
                Total = candidates.Length,
                Message = item.FileName
            });

            var categorized = Finalize(Categorize(item, request), rootPath, content, allowed);
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

            if (!content.CategorizeDirectories)
            {
                continue;
            }

            var archiveItem = Finalize(new CategorizedItem
            {
                FullPath = archive.RootPath,
                FileName = Path.GetFileName(archive.RootPath),
                Kind = EntryKind.Directory,
                Family = FileFamily.Archive,
                Category = "Archives",
                Subcategory = content.UseSubcategories ? "Projects" : null,
                SuggestedName = archive.SuggestedArchiveName,
                Rationale = archive.Reason,
                IsArchiveEntity = true,
                ArchiveFormat = archive.Format
            }, rootPath, content, allowed);
            entries.Add(archiveItem);
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
        var content = request.Content;
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

        if (item.Kind == EntryKind.File &&
            content.AnalyzeMedia &&
            FileFamilyClassifier.IsMedia(item.FileName))
        {
            string? lockReason = null;
            using var stream = LockTolerantFileAccess.TryOpenRead(item.FullPath, out lockReason);
            if (stream is null)
            {
                return HeuristicCategorizer.Categorize(item, request.Style) with
                {
                    IsLocked = true,
                    LockReason = lockReason,
                    Rationale = "Skipped media content analysis because the file is locked by another process."
                };
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

            return _mediaCategorizer.Categorize(item, metadata, content.OfferRenameMedia, request.Style);
        }

        var categorized = HeuristicCategorizer.Categorize(item, request.Style);
        if (item.Family == FileFamily.Document && content.AnalyzeDocuments)
        {
            categorized = categorized with
            {
                Rationale = "Document content analysis is queued for the selected LLM sidecar; extension-based category is used until that sidecar runs."
            };
        }

        if (item.Family == FileFamily.Image && content.AnalyzeImages)
        {
            categorized = categorized with
            {
                Rationale = "Picture content analysis is queued for the visual LLM sidecar; extension-based category is used until that sidecar runs."
            };
        }

        return categorized;
    }

    private static CategorizedItem Finalize(
        CategorizedItem item,
        string rootPath,
        ContentAnalysisOptions content,
        IReadOnlySet<string>? allowed)
    {
        var category = item.Category;
        var subcategory = content.UseSubcategories ? item.Subcategory : null;
        var suggestedName = item.SuggestedName;
        var rationale = item.Rationale;

        if (item.Family == FileFamily.Document && !content.OfferRenameDocuments)
        {
            suggestedName = null;
        }

        if (item.Family == FileFamily.Image && !content.OfferRenameImages)
        {
            suggestedName = null;
        }

        if (item.Family is FileFamily.Audio or FileFamily.Video && !content.OfferRenameMedia)
        {
            suggestedName = null;
        }

        if (item.Family == FileFamily.Image && content.AddImageDatePlaceToFilename)
        {
            suggestedName = PrefixDate(item.FullPath, suggestedName ?? item.FileName);
        }

        if (item.Family == FileFamily.Document && content.AddDocumentDateToCategory)
        {
            category = AppendDate(item.FullPath, category);
        }

        if (item.Family == FileFamily.Image && content.AddImageDateToCategory)
        {
            category = AppendDate(item.FullPath, category);
        }

        if (item.RenameOnly(content))
        {
            category = "Unchanged";
            subcategory = null;
            rationale = "Rename-only mode kept the current folder and suggested a new file name.";
        }

        if (allowed is not null && !allowed.Contains(category))
        {
            category = "Other";
            rationale = string.IsNullOrWhiteSpace(rationale)
                ? $"Whitelist '{content.ActiveWhitelist}' does not include the original category."
                : rationale + " Mapped to Other by the active whitelist.";
        }

        var finalized = item with
        {
            Category = category,
            Subcategory = subcategory,
            SuggestedName = suggestedName,
            Rationale = rationale,
            Status = SuggestionStatus.Local,
            Selected = true
        };
        return RelativePathComposer.WithLocalPath(finalized, rootPath, content);
    }

    private static bool ShouldInclude(ScannedItem item, ContentAnalysisOptions content)
    {
        if (item.Kind == EntryKind.Directory && !content.CategorizeDirectories)
        {
            return false;
        }

        if (item.Kind == EntryKind.File && !content.CategorizeFiles)
        {
            return false;
        }

        var documentsOnly = content.ProcessDocumentsOnly;
        var imagesOnly = content.ProcessImagesOnly;
        if (documentsOnly && imagesOnly)
        {
            return item.Kind == EntryKind.Directory || item.Family is FileFamily.Document or FileFamily.Image;
        }

        if (documentsOnly)
        {
            return item.Kind == EntryKind.Directory || item.Family == FileFamily.Document;
        }

        if (imagesOnly)
        {
            return item.Kind == EntryKind.Directory || item.Family == FileFamily.Image;
        }

        return true;
    }

    private static string AppendDate(string fullPath, string category)
    {
        try
        {
            return $"{category} {File.GetLastWriteTimeUtc(fullPath):yyyy}";
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return category;
        }
    }

    private static string PrefixDate(string fullPath, string fileName)
    {
        try
        {
            var stamp = File.GetLastWriteTimeUtc(fullPath).ToString("yyyy-MM-dd");
            if (fileName.StartsWith(stamp, StringComparison.Ordinal))
            {
                return fileName;
            }

            return stamp + "_" + fileName;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return fileName;
        }
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
