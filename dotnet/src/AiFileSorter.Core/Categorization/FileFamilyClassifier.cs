using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Categorization;

public sealed record MainCategorySelection(string FamilyName, IReadOnlyList<string> Categories);

/// <summary>Classifies files into the same families used by the Qt app.</summary>
public static class FileFamilyClassifier
{
    private static readonly HashSet<string> ImageExtensions =
    [
        ".jpg", ".jpeg", ".png", ".bmp", ".gif", ".webp", ".tif", ".tiff",
        ".tga", ".psd", ".hdr", ".pic", ".pnm", ".ppm", ".pgm", ".pbm",
        ".heic", ".heif", ".avif", ".ico", ".svg"
    ];

    private static readonly HashSet<string> DocumentExtensions =
    [
        ".txt", ".md", ".markdown", ".rtf", ".csv", ".tsv", ".log", ".json", ".xml", ".yml", ".yaml",
        ".ini", ".cfg", ".conf", ".html", ".htm", ".tex", ".rst", ".pdf", ".docx", ".xlsx", ".pptx",
        ".odt", ".ods", ".odp", ".doc", ".xls", ".ppt"
    ];

    private static readonly HashSet<string> PresentationExtensions = [".pptx", ".odp", ".ppt"];
    private static readonly HashSet<string> SpreadsheetExtensions = [".xlsx", ".ods", ".xls"];
    private static readonly HashSet<string> DataExportExtensions = [".csv", ".tsv"];
    private static readonly HashSet<string> ConfigExtensions = [".ini", ".cfg", ".conf"];

    private static readonly string[] SoftwareSuffixes =
    [
        ".exe", ".msi", ".msix", ".msixbundle", ".appx", ".appxbundle", ".deb", ".rpm", ".pkg",
        ".dmg", ".appimage", ".apk", ".run", ".bat", ".cmd", ".com"
    ];

    private static readonly string[] ArchiveSuffixes =
    [
        ".zip", ".7z", ".rar", ".tar", ".gz", ".bz2", ".xz", ".tgz", ".tbz", ".tbz2", ".txz",
        ".tar.gz", ".tar.bz2", ".tar.xz"
    ];

    private static readonly string[] AudioSuffixes =
    [
        ".aac", ".aif", ".aiff", ".alac", ".ape", ".flac", ".m4a", ".mp3", ".ogg", ".oga",
        ".opus", ".wav", ".wma"
    ];

    private static readonly string[] VideoSuffixes =
    [
        ".3gp", ".avi", ".flv", ".m4v", ".mkv", ".mov", ".mp4", ".mpeg", ".mpg", ".mts",
        ".m2ts", ".ts", ".webm", ".wmv"
    ];

    private static readonly string[] EbookSuffixes = [".epub", ".mobi", ".azw", ".azw3", ".fb2"];
    private static readonly string[] FontSuffixes = [".ttf", ".otf", ".woff", ".woff2"];

    public static FileFamily Classify(string fileName)
    {
        var extension = GetExtension(fileName);
        if (ImageExtensions.Contains(extension))
        {
            return FileFamily.Image;
        }

        if (DocumentExtensions.Contains(extension))
        {
            return FileFamily.Document;
        }

        var normalized = Normalize(fileName);
        if (EndsWithAny(normalized, SoftwareSuffixes))
        {
            return FileFamily.Software;
        }

        if (EndsWithAny(normalized, ArchiveSuffixes))
        {
            return FileFamily.Archive;
        }

        if (EndsWithAny(normalized, VideoSuffixes))
        {
            return FileFamily.Video;
        }

        if (EndsWithAny(normalized, AudioSuffixes))
        {
            return FileFamily.Audio;
        }

        if (EndsWithAny(normalized, EbookSuffixes))
        {
            return FileFamily.Ebook;
        }

        if (EndsWithAny(normalized, FontSuffixes))
        {
            return FileFamily.Font;
        }

        return FileFamily.Generic;
    }

    public static bool IsAudio(string fileName) => Classify(fileName) == FileFamily.Audio;
    public static bool IsVideo(string fileName) => Classify(fileName) == FileFamily.Video;
    public static bool IsMedia(string fileName)
    {
        var family = Classify(fileName);
        return family is FileFamily.Audio or FileFamily.Video;
    }

    public static MainCategorySelection DetermineMainCategories(string fileName, EntryKind kind)
    {
        if (kind != EntryKind.File)
        {
            return new MainCategorySelection("generic", GenericCategories());
        }

        return Classify(fileName) switch
        {
            FileFamily.Image => new MainCategorySelection("image", ["Images"]),
            FileFamily.Document => new MainCategorySelection("document", DocumentCategories(fileName)),
            FileFamily.Software => new MainCategorySelection("software", ["Software", "Installers", "Drivers", "Operating Systems", "Other"]),
            FileFamily.Archive => new MainCategorySelection("archive", ["Archives", "Software", "Data Exports", "Other"]),
            FileFamily.Audio => new MainCategorySelection("audio", ["Audio", "Other"]),
            FileFamily.Video => new MainCategorySelection("video", ["Videos", "Other"]),
            FileFamily.Ebook => new MainCategorySelection("ebook", ["Ebooks", "Documents", "Other"]),
            FileFamily.Font => new MainCategorySelection("font", ["Fonts", "Other"]),
            _ => new MainCategorySelection("generic", GenericCategories())
        };
    }

    public static string PreferredCategory(string fileName, FileFamily family, CategorizationStyle style)
    {
        return family switch
        {
            FileFamily.Image => "Images",
            FileFamily.Document => PreferredDocumentCategory(fileName),
            FileFamily.Software => "Software",
            FileFamily.Archive => "Archives",
            FileFamily.Audio => "Audio",
            FileFamily.Video => "Videos",
            FileFamily.Ebook => "Ebooks",
            FileFamily.Font => "Fonts",
            _ => style == CategorizationStyle.Consistent ? "Other" : "Unsorted"
        };
    }

    public static string PreferredDocumentCategory(string fileName)
    {
        var extension = GetExtension(fileName);
        if (PresentationExtensions.Contains(extension))
        {
            return "Presentations";
        }

        if (SpreadsheetExtensions.Contains(extension))
        {
            return "Spreadsheets";
        }

        if (DataExportExtensions.Contains(extension))
        {
            return "Data Exports";
        }

        if (ConfigExtensions.Contains(extension))
        {
            return "Configs";
        }

        return "Documents";
    }

    private static IReadOnlyList<string> DocumentCategories(string fileName) =>
    [
        PreferredDocumentCategory(fileName),
        "Documents",
        "Presentations",
        "Spreadsheets",
        "Data Exports",
        "Configs"
    ];

    private static IReadOnlyList<string> GenericCategories() =>
    [
        "Documents", "Images", "Videos", "Audio", "Software", "Archives",
        "Data Exports", "Configs", "Drivers", "Operating Systems", "Ebooks",
        "Fonts", "Other"
    ];

    public static string GetExtension(string fileName)
    {
        var normalized = Normalize(fileName);
        var newline = normalized.IndexOf('\n');
        if (newline >= 0)
        {
            normalized = normalized[..newline];
        }

        foreach (var compound in new[] { ".tar.gz", ".tar.bz2", ".tar.xz" })
        {
            if (normalized.EndsWith(compound, StringComparison.Ordinal))
            {
                return compound;
            }
        }

        var dot = normalized.LastIndexOf('.');
        return dot < 0 || dot == normalized.Length - 1 ? "" : normalized[dot..];
    }

    private static string Normalize(string fileName)
    {
        var newline = fileName.IndexOf('\n');
        var value = newline >= 0 ? fileName[..newline] : fileName;
        return value.ToLowerInvariant();
    }

    private static bool EndsWithAny(string value, IReadOnlyList<string> suffixes)
    {
        foreach (var suffix in suffixes)
        {
            if (value.EndsWith(suffix, StringComparison.Ordinal))
            {
                return true;
            }
        }

        return false;
    }
}
