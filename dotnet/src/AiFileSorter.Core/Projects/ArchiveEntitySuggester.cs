using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Projects;

/// <summary>Turns detected project folders into zip/tar suggestions that should be filed as one entity.</summary>
public sealed class ArchiveEntitySuggester
{
    public ArchiveEntitySuggestion? Suggest(ProjectMatch match)
    {
        var format = SuggestFormat(match.Id);
        var folderName = Path.GetFileName(match.RootPath.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar));
        if (string.IsNullOrWhiteSpace(folderName))
        {
            folderName = match.Id;
        }

        var extension = format == ArchiveFormat.Zip ? ".zip" : ".tar.gz";
        var archiveName = SanitizeFileName(folderName) + extension;
        var reason = match.Strength == ProjectStrength.Strong
            ? $"{match.Name} should stay together. Archive as {archiveName} instead of moving child files."
            : $"{match.Name} is a weak project signal. Consider archiving as {archiveName} if the folder is a self-contained scene.";

        return new ArchiveEntitySuggestion(
            match.RootPath,
            match.Id,
            match.Name,
            format,
            archiveName,
            reason,
            match.Strength);
    }

    public static ArchiveFormat SuggestFormat(string projectId)
    {
        return projectId switch
        {
            "git" or "python" or "rust" or "go" or "node" or "gradle" => ArchiveFormat.TarGz,
            _ => ArchiveFormat.Zip
        };
    }

    private static string SanitizeFileName(string value)
    {
        var invalid = Path.GetInvalidFileNameChars();
        var chars = value.Select(ch => invalid.Contains(ch) ? '_' : ch).ToArray();
        var cleaned = new string(chars).Trim();
        return string.IsNullOrWhiteSpace(cleaned) ? "project" : cleaned;
    }
}
