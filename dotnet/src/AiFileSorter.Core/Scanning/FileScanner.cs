using AiFileSorter.Core.Categorization;
using AiFileSorter.Core.Models;
using AiFileSorter.Core.Projects;

namespace AiFileSorter.Core.Scanning;

/// <summary>Enumerates a folder without taking exclusive file locks or aborting on unreadable children.</summary>
public sealed class FileScanner
{
    private static readonly HashSet<string> JunkNames = new(StringComparer.OrdinalIgnoreCase)
    {
        "thumbs.db", "desktop.ini", ".ds_store", "ehthumbs.db"
    };

    private readonly ProjectDetector _projectDetector;
    private readonly ArchiveEntitySuggester _archiveSuggester;

    public FileScanner()
        : this(new ProjectDetector(), new ArchiveEntitySuggester())
    {
    }

    public FileScanner(ProjectDetector projectDetector, ArchiveEntitySuggester archiveSuggester)
    {
        _projectDetector = projectDetector;
        _archiveSuggester = archiveSuggester;
    }

    public ScanResult Scan(string directoryPath, ScanOptions options, CancellationToken cancellationToken = default)
    {
        var items = new List<ScannedItem>();
        var archives = new List<ArchiveEntitySuggestion>();
        var warnings = new List<string>();
        var root = Path.GetFullPath(directoryPath);

        if (!Directory.Exists(root))
        {
            throw new DirectoryNotFoundException($"Directory not found: {root}");
        }

        var rootMatch = _projectDetector.Detect(root);
        if (rootMatch is not null)
        {
            MaybeAddArchive(rootMatch, options, archives);
            if (options.ProtectProjectDirectories && rootMatch.ShouldSkipTraversal)
            {
                items.Add(ToArchiveItem(rootMatch, archives.Last()));
                return new ScanResult(items, archives, warnings);
            }
        }

        if (options.Recursive)
        {
            ScanRecursive(root, options, items, archives, warnings, cancellationToken);
        }
        else
        {
            ScanOneDirectory(root, options, items, archives, warnings, recurse: false, cancellationToken);
        }

        return new ScanResult(items, archives, warnings);
    }

    private void ScanRecursive(
        string root,
        ScanOptions options,
        List<ScannedItem> items,
        List<ArchiveEntitySuggestion> archives,
        List<string> warnings,
        CancellationToken cancellationToken)
    {
        var pending = new Stack<string>();
        pending.Push(root);

        while (pending.Count > 0)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var current = pending.Pop();
            var children = ScanOneDirectory(current, options, items, archives, warnings, recurse: true, cancellationToken);
            foreach (var child in children)
            {
                pending.Push(child);
            }
        }
    }

    private List<string> ScanOneDirectory(
        string directory,
        ScanOptions options,
        List<ScannedItem> items,
        List<ArchiveEntitySuggestion> archives,
        List<string> warnings,
        bool recurse,
        CancellationToken cancellationToken)
    {
        var nestedDirectories = new List<string>();
        IEnumerable<string> entries;
        try
        {
            entries = Directory.EnumerateFileSystemEntries(directory);
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or IOException)
        {
            warnings.Add($"Skipping unreadable directory '{directory}': {ex.Message}");
            return nestedDirectories;
        }

        foreach (var entry in entries)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var name = Path.GetFileName(entry);
            if (ShouldSkipName(name, options))
            {
                continue;
            }

            bool isDirectory;
            bool isReparse;
            try
            {
                var attrs = File.GetAttributes(entry);
                isDirectory = attrs.HasFlag(FileAttributes.Directory);
                isReparse = attrs.HasFlag(FileAttributes.ReparsePoint);
            }
            catch (Exception ex) when (ex is UnauthorizedAccessException or IOException)
            {
                warnings.Add($"Skipping unreadable entry '{entry}': {ex.Message}");
                continue;
            }

            if (options.SkipReparsePoints && isReparse)
            {
                continue;
            }

            if (!options.IncludeHidden && IsHidden(entry, name, isDirectory))
            {
                continue;
            }

            if (isDirectory)
            {
                var match = _projectDetector.Detect(entry);
                if (match is not null)
                {
                    MaybeAddArchive(match, options, archives);
                    if (options.ProtectProjectDirectories && match.ShouldSkipTraversal)
                    {
                        if (options.IncludeDirectories)
                        {
                            items.Add(ToArchiveItem(match, archives.Count > 0 ? archives.Last() : null));
                        }

                        continue;
                    }
                }

                if (options.IncludeDirectories)
                {
                    items.Add(new ScannedItem(entry, name, EntryKind.Directory, FileFamily.Generic, false, null));
                }

                if (recurse)
                {
                    nestedDirectories.Add(entry);
                }

                continue;
            }

            if (!options.IncludeFiles)
            {
                continue;
            }

            items.Add(new ScannedItem(
                entry,
                name,
                EntryKind.File,
                FileFamilyClassifier.Classify(name),
                false,
                null));
        }

        return nestedDirectories;
    }

    private void MaybeAddArchive(ProjectMatch match, ScanOptions options, List<ArchiveEntitySuggestion> archives)
    {
        if (!options.SuggestArchiveEntities)
        {
            return;
        }

        var suggestion = _archiveSuggester.Suggest(match);
        if (suggestion is not null)
        {
            archives.Add(suggestion);
        }
    }

    private static ScannedItem ToArchiveItem(ProjectMatch match, ArchiveEntitySuggestion? suggestion)
    {
        return new ScannedItem(
            match.RootPath,
            Path.GetFileName(match.RootPath),
            EntryKind.Directory,
            FileFamily.Archive,
            false,
            suggestion?.Reason);
    }

    private static bool ShouldSkipName(string name, ScanOptions options)
    {
        if (JunkNames.Contains(name))
        {
            return true;
        }

        foreach (var extra in options.AdditionalJunkNames)
        {
            if (name.Equals(extra, StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }

        foreach (var prefix in options.JunkNamePrefixes)
        {
            if (name.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }

        return false;
    }

    private static bool IsHidden(string path, string name, bool isDirectory)
    {
        if (name.StartsWith('.'))
        {
            return true;
        }

        try
        {
            var attrs = File.GetAttributes(path);
            return attrs.HasFlag(FileAttributes.Hidden);
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or IOException)
        {
            return isDirectory;
        }
    }
}

public sealed record ScanResult(
    IReadOnlyList<ScannedItem> Items,
    IReadOnlyList<ArchiveEntitySuggestion> ArchiveEntities,
    IReadOnlyList<string> Warnings);
