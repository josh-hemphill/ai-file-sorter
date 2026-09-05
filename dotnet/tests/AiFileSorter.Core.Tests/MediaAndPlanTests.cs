using System.Text;
using AiFileSorter.Core.Engine;
using AiFileSorter.Core.IO;
using AiFileSorter.Core.Media;
using AiFileSorter.Core.Models;
using AiFileSorter.Core.Persistence;
using AiFileSorter.Core.Plans;
using AiFileSorter.Core.Scanning;
using Xunit;

namespace AiFileSorter.Core.Tests;

public sealed class MediaAndPlanTests
{
    [Fact]
    public void Reads_id3v1_tags()
    {
        var bytes = new byte[256];
        Encoding.ASCII.GetBytes("ID3").CopyTo(bytes, 0);
        var tag = bytes.AsSpan(bytes.Length - 128);
        Encoding.ASCII.GetBytes("TAG").CopyTo(tag);
        Encoding.Latin1.GetBytes("Moon Tides").CopyTo(tag[3..]);
        Encoding.Latin1.GetBytes("Synth Unit").CopyTo(tag[33..]);
        Encoding.Latin1.GetBytes("Celestial").CopyTo(tag[63..]);
        Encoding.ASCII.GetBytes("2021").CopyTo(tag[93..]);

        var metadata = MediaMetadataReader.ParseId3v1(bytes);
        Assert.Equal("Moon Tides", metadata?.Title);
        Assert.Equal("Synth Unit", metadata?.Artist);
        Assert.Equal("Celestial", metadata?.Album);
        Assert.Equal("2021", metadata?.Year);
    }

    [Fact]
    public void Reads_id3v2_text_frames()
    {
        var metadata = MediaMetadataReader.ParseId3(CreateId3v24(
            ("TIT2", "Night Drive"),
            ("TPE1", "Neon City"),
            ("TALB", "After Hours"),
            ("TDRC", "2024-05-01"),
            ("TCON", "Podcast")));
        Assert.Equal("Night Drive", metadata?.Title);
        Assert.Equal("Neon City", metadata?.Artist);
        Assert.Equal("After Hours", metadata?.Album);
        Assert.Equal("2024", metadata?.Year);
        Assert.Equal("Podcast", metadata?.Genre);
    }

    [Fact]
    public void Categorizes_podcast_and_screen_recording_from_content_cues()
    {
        var categorizer = new MediaContentCategorizer();
        var podcast = categorizer.Categorize(
            new ScannedItem("/tmp/show.mp3", "weekly_podcast_episode_12.mp3", EntryKind.File, FileFamily.Audio, false, null),
            new MediaMetadata { Title = "Episode 12", Genre = "Podcast" },
            suggestRename: true,
            CategorizationStyle.Refined);
        var capture = categorizer.Categorize(
            new ScannedItem("/tmp/obs.mp4", "OBS_screen_recording.mp4", EntryKind.File, FileFamily.Video, false, null),
            null,
            suggestRename: false,
            CategorizationStyle.Refined);

        Assert.Equal("Audio", podcast.Category);
        Assert.Equal("Podcasts", podcast.Subcategory);
        Assert.Equal("2024_neon_city_after_hours_night_drive.mp3", MediaRenameComposer.Compose("clip.mp3", new MediaMetadata
        {
            Year = "2024",
            Artist = "Neon City",
            Album = "After Hours",
            Title = "Night Drive"
        }));
        Assert.Equal("Videos", capture.Category);
        Assert.Equal("Screen Recordings", capture.Subcategory);
    }

    [Fact]
    public void Camera_and_tv_filenames_get_video_subcategories()
    {
        var categorizer = new MediaContentCategorizer();
        var camera = categorizer.Categorize(
            new ScannedItem("/tmp/DJI_0042.mp4", "DJI_0042.mp4", EntryKind.File, FileFamily.Video, false, null),
            null,
            false,
            CategorizationStyle.Refined);
        var tv = categorizer.Categorize(
            new ScannedItem("/tmp/Show.S01E03.mkv", "Show.S01E03.mkv", EntryKind.File, FileFamily.Video, false, null),
            null,
            false,
            CategorizationStyle.Refined);

        Assert.Equal("Camera Footage", camera.Subcategory);
        Assert.Equal("TV", tv.Subcategory);
    }

    [Fact]
    public async Task Engine_builds_plan_and_sqlite_round_trips()
    {
        using var root = new TempFolder();
        File.WriteAllText(Path.Combine(root.Path, "notes.md"), "# hello");
        File.WriteAllText(Path.Combine(root.Path, "song.mp3"), "not a real mp3");
        File.WriteAllText(Path.Combine(root.Path, "weekly_podcast_episode.mp3"), "audio");
        Write(root.Path, "UnityGame/Assets/player.cs", "class Player {}");
        Write(root.Path, "UnityGame/ProjectSettings/ProjectVersion.txt", "2019");
        var dbPath = Path.Combine(root.Path, "suggestions.db");

        var engine = new AnalysisEngine();
        var plan = await engine.AnalyzeAsync(new AnalysisRequest
        {
            RootPath = root.Path,
            Scan = ScanOptions.DefaultRecursive(),
            DatabasePath = dbPath
        }, progress: null);

        Assert.Contains(plan.Entries, entry => entry.FileName == "notes.md" && entry.Category == "Documents");
        Assert.Contains(plan.Entries, entry => entry.FileName.Contains("podcast") && entry.Subcategory == "Podcasts");
        Assert.Contains(plan.ArchiveEntities, entity => entity.ProjectId == "unity" && entity.Format == ArchiveFormat.Zip);
        Assert.DoesNotContain(plan.Entries, entry => entry.FileName == "player.cs");

        using var store = new SuggestionStore(dbPath);
        var loaded = store.LoadLatestPlan(root.Path);
        Assert.NotNull(loaded);
        Assert.Equal(plan.PlanId, loaded!.PlanId);
        Assert.Equal(plan.Summary.ArchiveEntityCount, loaded.Summary.ArchiveEntityCount);
    }

    [Fact]
    public void Remote_handoff_prompt_includes_archive_entities_and_parses_model_json()
    {
        var plan = new FilingPlan
        {
            RootPath = "/tmp/inbox",
            Entries =
            [
                new CategorizedItem
                {
                    FullPath = "/tmp/inbox/show.mp3",
                    FileName = "show.mp3",
                    Kind = EntryKind.File,
                    Family = FileFamily.Audio,
                    Category = "Audio",
                    Subcategory = "Podcasts",
                    Rationale = "podcast"
                }
            ],
            ArchiveEntities =
            [
                new ArchiveEntitySuggestion("/tmp/inbox/Game", "unity", "Unity project", ArchiveFormat.Zip, "Game.zip", "keep together", ProjectStrength.Strong)
            ],
            Summary = new FilingPlanSummary { FileCount = 1, ArchiveEntityCount = 1 }
        };

        var payload = new RemotePlanHandoff().Create(plan);
        Assert.Contains("Keep project folders", payload.CompactPrompt);
        Assert.Contains("Game.zip", payload.CompactJson);
        Assert.Contains("Podcasts", payload.CompactJson);

        var parsed = RemotePlanHandoff.ParseModelJson("""
            Here you go:
            {"proposedRootName":"Media Library","rationale":"Group by kind.","folders":[{"category":"Audio","subcategory":"Podcasts","notes":"Keep episodes together"}]}
            """);
        Assert.Equal("Media Library", parsed.ProposedRootName);
        Assert.Equal("Podcasts", parsed.Folders[0].Subcategory);
    }

    [Fact]
    public void Shared_read_opens_normal_files_without_exclusive_lock()
    {
        using var root = new TempFolder();
        var path = Path.Combine(root.Path, "open.txt");
        File.WriteAllText(path, "hello");
        using var first = LockTolerantFileAccess.TryOpenRead(path, out var firstReason);
        using var second = LockTolerantFileAccess.TryOpenRead(path, out var secondReason);
        Assert.NotNull(first);
        Assert.NotNull(second);
        Assert.Null(firstReason);
        Assert.Null(secondReason);
    }

    private static byte[] CreateId3v24(params (string Id, string Value)[] frames)
    {
        using var body = new MemoryStream();
        foreach (var (id, value) in frames)
        {
            var payload = new List<byte> { 3 };
            payload.AddRange(Encoding.UTF8.GetBytes(value));
            payload.Add(0);
            body.Write(Encoding.ASCII.GetBytes(id));
            body.WriteByte(0);
            body.WriteByte(0);
            body.WriteByte(0);
            body.WriteByte((byte)payload.Count);
            body.WriteByte(0);
            body.WriteByte(0);
            body.Write(payload.ToArray());
        }

        var frameBytes = body.ToArray();
        var header = new byte[10];
        header[0] = (byte)'I';
        header[1] = (byte)'D';
        header[2] = (byte)'3';
        header[3] = 4;
        header[4] = 0;
        header[5] = 0;
        WriteSynchsafe(header, 6, frameBytes.Length);
        return header.Concat(frameBytes).ToArray();
    }

    private static void WriteSynchsafe(byte[] target, int offset, int value)
    {
        target[offset] = (byte)((value >> 21) & 0x7F);
        target[offset + 1] = (byte)((value >> 14) & 0x7F);
        target[offset + 2] = (byte)((value >> 7) & 0x7F);
        target[offset + 3] = (byte)(value & 0x7F);
    }

    private static void Write(string root, string relative, string contents)
    {
        var path = Path.Combine(root, relative);
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, contents);
    }
}

public sealed class TempFolder : IDisposable
{
    public string Path { get; } = System.IO.Path.Combine(System.IO.Path.GetTempPath(), "aifs-tests-" + Guid.NewGuid().ToString("N"));

    public TempFolder()
    {
        Directory.CreateDirectory(Path);
    }

    public void Dispose()
    {
        try
        {
            Directory.Delete(Path, recursive: true);
        }
        catch (IOException)
        {
        }
    }
}
