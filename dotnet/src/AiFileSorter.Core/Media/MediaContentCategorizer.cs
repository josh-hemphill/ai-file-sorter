using System.Text;
using AiFileSorter.Core.Categorization;
using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Media;

/// <summary>Categorizes audio and video from tags, duration, and filename content cues.</summary>
public sealed class MediaContentCategorizer
{
    public CategorizedItem Categorize(
        ScannedItem item,
        MediaMetadata? metadata,
        bool suggestRename,
        CategorizationStyle style)
    {
        var fileName = item.FileName;
        var stem = Path.GetFileNameWithoutExtension(fileName).ToLowerInvariant();
        var genre = metadata?.Genre?.ToLowerInvariant() ?? "";
        var title = metadata?.Title?.ToLowerInvariant() ?? "";
        var combined = $"{stem} {title} {genre} {metadata?.Album} {metadata?.Artist}".ToLowerInvariant();

        string category;
        string subcategory;
        string rationale;

        if (item.Family == FileFamily.Audio)
        {
            category = "Audio";
            (subcategory, rationale) = ClassifyAudio(combined, genre, metadata, style);
        }
        else
        {
            category = "Videos";
            (subcategory, rationale) = ClassifyVideo(combined, metadata, style);
        }

        return new CategorizedItem
        {
            FullPath = item.FullPath,
            FileName = item.FileName,
            Kind = item.Kind,
            Family = item.Family,
            Category = category,
            Subcategory = subcategory,
            SuggestedName = suggestRename ? MediaRenameComposer.Compose(item.FileName, metadata) : null,
            Rationale = rationale,
            Media = metadata,
            IsLocked = item.IsLocked,
            LockReason = item.LockReason
        };
    }

    private static (string Subcategory, string Rationale) ClassifyAudio(
        string combined,
        string genre,
        MediaMetadata? metadata,
        CategorizationStyle style)
    {
        if (ContainsAny(combined, "podcast", "episode", "ep."))
        {
            return ("Podcasts", "Title or filename looks like a podcast episode.");
        }

        if (ContainsAny(combined, "audiobook", "audio book") ||
            (metadata?.DurationSeconds >= 30 * 60 && ContainsAny(genre, "speech", "spoken", "audiobook")))
        {
            return ("Audiobooks", "Long spoken-word audio is treated as an audiobook.");
        }

        if (ContainsAny(combined, "sfx", "sound effect", "foley") || ContainsAny(genre, "sound effect"))
        {
            return ("Sound Effects", "Filename or genre identifies sound-design audio.");
        }

        if (ContainsAny(combined, "voicemail", "voice memo", "voice note", "recording"))
        {
            return ("Voice Notes", "Filename looks like a personal voice recording.");
        }

        if (ContainsAny(combined, "lecture", "sermon", "interview") || ContainsAny(genre, "speech", "spoken"))
        {
            return ("Spoken Word", "Spoken-word cues were found in tags or the filename.");
        }

        if (ContainsAny(genre, "soundtrack") || ContainsAny(combined, "ost", "soundtrack"))
        {
            return ("Soundtracks", "Genre or filename points at a soundtrack.");
        }

        if (metadata?.HasNamingParts == true)
        {
            return ("Music", "Embedded artist/album/title tags look like a music track.");
        }

        if (style == CategorizationStyle.Consistent)
        {
            return ("Music", "Consistent mode keeps untagged audio under Music.");
        }

        return ("Unsorted", "No strong audio content cues were available.");
    }

    private static (string Subcategory, string Rationale) ClassifyVideo(
        string combined,
        MediaMetadata? metadata,
        CategorizationStyle style)
    {
        if (ContainsAny(combined, "screen recording", "screenrecording", "screencast", "obs", "capture"))
        {
            return ("Screen Recordings", "Filename matches common screen-capture patterns.");
        }

        if (ContainsAny(combined, "tutorial", "howto", "how to", "lesson", "course"))
        {
            return ("Tutorials", "Filename looks like instructional video.");
        }

        if (ContainsAny(combined, "dji_", "gopr", "img_", "vid_", "mvi_", "dscn") ||
            ContainsAny(combined, "gopro", "dashcam"))
        {
            return ("Camera Footage", "Filename matches camera or drone still/video naming.");
        }

        if (ContainsAny(combined, "s01e", "s1e", "season", "episode") || LooksLikeTvEpisode(combined))
        {
            return ("TV", "Filename looks like a TV episode.");
        }

        if (metadata?.DurationSeconds >= 60 * 60 || ContainsAny(combined, "movie", "film"))
        {
            return ("Movies", "Long runtime or movie-like naming.");
        }

        if (metadata?.DurationSeconds is > 0 and < 5 * 60)
        {
            return ("Clips", "Short video duration.");
        }

        if (style == CategorizationStyle.Consistent)
        {
            return ("Videos", "Consistent mode keeps untagged video under Videos.");
        }

        return ("Unsorted", "No strong video content cues were available.");
    }

    private static bool LooksLikeTvEpisode(string value)
    {
        return System.Text.RegularExpressions.Regex.IsMatch(
            value,
            @"\b(?:s\d{1,2}e\d{1,3}|\d{1,2}x\d{1,3})\b",
            System.Text.RegularExpressions.RegexOptions.IgnoreCase | System.Text.RegularExpressions.RegexOptions.CultureInvariant);
    }

    private static bool ContainsAny(string value, params string[] needles)
    {
        foreach (var needle in needles)
        {
            if (value.Contains(needle, StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }

        return false;
    }
}

public static class MediaRenameComposer
{
    public static string? Compose(string originalFileName, MediaMetadata? metadata)
    {
        if (metadata is null || !metadata.HasNamingParts)
        {
            return null;
        }

        var parts = new[] { metadata.Year, metadata.Artist, metadata.Album, metadata.Title }
            .Where(part => !string.IsNullOrWhiteSpace(part))
            .Select(Slugify)
            .Where(part => !string.IsNullOrWhiteSpace(part));
        var stem = string.Join('_', parts);
        if (string.IsNullOrWhiteSpace(stem))
        {
            return null;
        }

        var extension = Path.GetExtension(originalFileName);
        return stem + extension.ToLowerInvariant();
    }

    private static string Slugify(string? value)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            return "";
        }

        var builder = new StringBuilder();
        var previousUnderscore = false;
        foreach (var ch in value.Trim().ToLowerInvariant())
        {
            if (char.IsLetterOrDigit(ch))
            {
                builder.Append(ch);
                previousUnderscore = false;
                continue;
            }

            if (!previousUnderscore)
            {
                builder.Append('_');
                previousUnderscore = true;
            }
        }

        return builder.ToString().Trim('_');
    }
}
