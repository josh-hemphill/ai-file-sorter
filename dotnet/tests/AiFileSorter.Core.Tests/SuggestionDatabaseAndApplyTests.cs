using AiFileSorter.Core.Engine;
using AiFileSorter.Core.Llm;
using AiFileSorter.Core.Models;
using AiFileSorter.Core.Persistence;
using AiFileSorter.Core.Plans;
using Xunit;

namespace AiFileSorter.Core.Tests;

public sealed class SuggestionDatabaseAndApplyTests
{
    [Fact]
    public async Task Documents_only_omits_media_and_can_drop_subcategories()
    {
        using var root = new TempFolder();
        File.WriteAllText(Path.Combine(root.Path, "notes.md"), "# hello");
        File.WriteAllText(Path.Combine(root.Path, "song.mp3"), "audio");
        File.WriteAllText(Path.Combine(root.Path, "photo.png"), "png");

        var plan = await new AnalysisEngine().AnalyzeAsync(new AnalysisRequest
        {
            RootPath = root.Path,
            Scan = ScanOptions.DefaultRecursive(),
            Content = new ContentAnalysisOptions
            {
                ProcessDocumentsOnly = true,
                UseSubcategories = false,
                AnalyzeMedia = false
            }
        }, progress: null);

        Assert.Contains(plan.Entries, entry => entry.FileName == "notes.md");
        Assert.DoesNotContain(plan.Entries, entry => entry.FileName == "song.mp3");
        Assert.DoesNotContain(plan.Entries, entry => entry.FileName == "photo.png");
        Assert.All(plan.Entries.Where(entry => entry.Kind == EntryKind.File), entry => Assert.Null(entry.Subcategory));
        Assert.Contains(plan.Entries, entry => entry.FileName == "notes.md" && entry.LocalRelativePath == "Documents/notes.md");
    }

    [Fact]
    public async Task Sqlite_round_trips_suggestion_rows()
    {
        using var root = new TempFolder();
        File.WriteAllText(Path.Combine(root.Path, "notes.md"), "# hello");
        var dbPath = Path.Combine(root.Path, "suggestions.sqlite");

        var engine = new AnalysisEngine();
        var plan = await engine.AnalyzeAsync(new AnalysisRequest
        {
            RootPath = root.Path,
            Scan = ScanOptions.DefaultRecursive(),
            DatabasePath = dbPath
        }, progress: null);

        using var store = new SuggestionStore(dbPath);
        var loaded = store.LoadLatestPlan(root.Path);
        Assert.NotNull(loaded);
        Assert.Equal(plan.PlanId, loaded!.PlanId);
        Assert.Contains(loaded.Entries, entry => entry.LocalRelativePath != null && entry.LocalRelativePath.Contains("notes.md"));
    }

    [Fact]
    public void Remote_merge_writes_proposed_paths_without_moving_files()
    {
        using var root = new TempFolder();
        var source = Path.Combine(root.Path, "show.mp3");
        File.WriteAllText(source, "audio");
        var plan = new FilingPlan
        {
            RootPath = root.Path,
            Entries =
            [
                new CategorizedItem
                {
                    FullPath = source,
                    FileName = "show.mp3",
                    Kind = EntryKind.File,
                    Family = FileFamily.Audio,
                    Category = "Audio",
                    Subcategory = "Podcasts",
                    LocalRelativePath = "Audio/Podcasts/show.mp3",
                    Status = SuggestionStatus.Local
                }
            ]
        };

        var merged = RemoteProposalMerger.Merge(plan, new RemoteStructureSuggestion
        {
            Rationale = "Group episodes",
            Updates =
            [
                new RemotePathUpdate
                {
                    FullPath = source,
                    FileName = "show.mp3",
                    ProposedRelativePath = "Media/Podcasts/Weekly/show.mp3",
                    Category = "Audio",
                    Subcategory = "Podcasts",
                    Rationale = "Keep episodes together"
                }
            ]
        });

        Assert.Equal("Media/Podcasts/Weekly/show.mp3", merged.Entries[0].RemoteRelativePath);
        Assert.Equal(SuggestionStatus.RemoteProposed, merged.Entries[0].Status);
        Assert.True(File.Exists(source));
    }

    [Fact]
    public void Apply_dry_run_does_not_move_files_and_apply_moves_accepted_rows()
    {
        using var root = new TempFolder();
        var source = Path.Combine(root.Path, "notes.md");
        File.WriteAllText(source, "hello");
        var plan = new FilingPlan
        {
            RootPath = root.Path,
            Entries =
            [
                new CategorizedItem
                {
                    FullPath = source,
                    FileName = "notes.md",
                    Kind = EntryKind.File,
                    Family = FileFamily.Document,
                    Category = "Documents",
                    LocalRelativePath = "notes.md",
                    AcceptedRelativePath = "Documents/notes.md",
                    Status = SuggestionStatus.Accepted,
                    Selected = true
                }
            ]
        };

        var engine = new AnalysisEngine();
        var preview = engine.PreviewApply(plan);
        Assert.Single(preview.Moves);
        Assert.True(File.Exists(source));

        var result = engine.Apply(plan, dryRun: false, databasePath: null);
        Assert.Single(result.Applied);
        Assert.False(File.Exists(source));
        Assert.True(File.Exists(Path.Combine(root.Path, "Documents", "notes.md")));
        Assert.False(string.IsNullOrWhiteSpace(result.UndoPlanPath));
    }

    [Fact]
    public void Llm_catalog_matches_upstream_model_names()
    {
        Assert.Contains(LlmCatalog.BuiltinLocalModels, entry => entry.DisplayName.Contains("Gemma 3 4B IT"));
        Assert.Contains(LlmCatalog.BuiltinLocalModels, entry => entry.DisplayName.Contains("Mistral 7B"));
        Assert.Contains(LlmCatalog.VisualBackends, entry => entry.Id == "llava-v1.6-mistral-7b");
        Assert.Contains(LlmCatalog.RemoteEndpoints, entry => entry.Kind == LlmKind.OpenAi);
    }

    [Fact]
    public void Rejects_parent_directory_escape_in_remote_paths()
    {
        Assert.Throws<InvalidOperationException>(() =>
            RelativePathComposer.SanitizeRelativePath("../outside.txt"));
    }
}
