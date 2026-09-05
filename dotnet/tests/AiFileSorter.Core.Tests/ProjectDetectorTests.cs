using AiFileSorter.Core.Categorization;
using AiFileSorter.Core.Models;
using AiFileSorter.Core.Projects;
using AiFileSorter.Core.Scanning;
using Xunit;

namespace AiFileSorter.Core.Tests;

public sealed class ProjectDetectorTests
{
    [Theory]
    [InlineData("unity", "Assets", "ProjectSettings/ProjectVersion.txt")]
    [InlineData("python", "pyproject.toml")]
    [InlineData("rust", "Cargo.toml")]
    [InlineData("go", "go.mod")]
    [InlineData("godot", "project.godot")]
    public void Detects_strong_project_markers(string expectedId, params string[] files)
    {
        using var root = new TempFolder();
        foreach (var relative in files)
        {
            Write(root.Path, relative, "x");
        }

        var match = new ProjectDetector().Detect(root.Path);
        Assert.NotNull(match);
        Assert.Equal(expectedId, match!.Id);
        Assert.Equal(ProjectStrength.Strong, match.Strength);
        Assert.True(match.ShouldSkipTraversal);
    }

    [Fact]
    public void Detects_node_project_with_lockfile()
    {
        using var root = new TempFolder();
        Write(root.Path, "package.json", "{}");
        Write(root.Path, "pnpm-lock.yaml", "lockfileVersion: 9");

        var match = new ProjectDetector().Detect(root.Path);
        Assert.Equal("node", match?.Id);
    }

    [Fact]
    public void Detects_unreal_project()
    {
        using var root = new TempFolder();
        Directory.CreateDirectory(Path.Combine(root.Path, "Config"));
        Directory.CreateDirectory(Path.Combine(root.Path, "Content"));
        Write(root.Path, "Game.uproject", "{}");

        var match = new ProjectDetector().Detect(root.Path);
        Assert.Equal("unreal", match?.Id);
    }

    [Fact]
    public void Detects_dotnet_solution_suffix()
    {
        using var root = new TempFolder();
        Write(root.Path, "App.sln", "");

        var match = new ProjectDetector().Detect(root.Path);
        Assert.Equal("dotnet", match?.Id);
    }

    [Fact]
    public void Lone_blend_file_is_weak()
    {
        using var root = new TempFolder();
        Write(root.Path, "scene.blend", "data");

        var match = new ProjectDetector().Detect(root.Path);
        Assert.Equal("blender-file", match?.Id);
        Assert.Equal(ProjectStrength.Weak, match?.Strength);
        Assert.False(match!.ShouldSkipTraversal);
    }

    [Fact]
    public void Blender_with_textures_is_strong()
    {
        using var root = new TempFolder();
        Write(root.Path, "scene.blend", "data");
        Write(root.Path, "textures/wood.png", "img");

        var match = new ProjectDetector().Detect(root.Path);
        Assert.Equal("blender", match?.Id);
        Assert.Equal(ProjectStrength.Strong, match?.Strength);
    }

    [Fact]
    public void Suggests_tar_for_source_projects_and_zip_for_game_projects()
    {
        var suggester = new ArchiveEntitySuggester();
        var rust = suggester.Suggest(new ProjectMatch("/tmp/repo", "rust", "Rust project", ProjectStrength.Strong, "reason"));
        var unity = suggester.Suggest(new ProjectMatch("/tmp/game", "unity", "Unity project", ProjectStrength.Strong, "reason"));

        Assert.Equal(ArchiveFormat.TarGz, rust?.Format);
        Assert.Equal("repo.tar.gz", rust?.SuggestedArchiveName);
        Assert.Equal(ArchiveFormat.Zip, unity?.Format);
        Assert.Equal("game.zip", unity?.SuggestedArchiveName);
    }

    [Fact]
    public void Scanner_skips_nested_project_children_and_emits_archive_suggestion()
    {
        using var root = new TempFolder();
        Write(root.Path, "notes.txt", "hello");
        Write(root.Path, "UnityGame/Assets/player.cs", "class Player {}");
        Write(root.Path, "UnityGame/ProjectSettings/ProjectVersion.txt", "2019");

        var result = new FileScanner().Scan(root.Path, ScanOptions.DefaultRecursive());
        Assert.DoesNotContain(result.Items, item => item.FileName == "player.cs");
        Assert.Contains(result.ArchiveEntities, entity => entity.ProjectId == "unity");
        Assert.Contains(result.Items, item => item.FileName == "UnityGame");
    }

    private static void Write(string root, string relative, string contents)
    {
        var path = Path.Combine(root, relative);
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, contents);
    }
}

public sealed class FileFamilyClassifierTests
{
    [Theory]
    [InlineData("song.mp3", FileFamily.Audio)]
    [InlineData("clip.MP4", FileFamily.Video)]
    [InlineData("photo.heic", FileFamily.Image)]
    [InlineData("report.docx", FileFamily.Document)]
    [InlineData("archive.tar.gz", FileFamily.Archive)]
    [InlineData("setup.exe", FileFamily.Software)]
    public void Classifies_known_extensions(string fileName, FileFamily expected)
    {
        Assert.Equal(expected, FileFamilyClassifier.Classify(fileName));
    }

    [Fact]
    public void Presentation_files_prefer_presentations_category()
    {
        Assert.Equal("Presentations", FileFamilyClassifier.PreferredDocumentCategory("deck.pptx"));
        Assert.Equal("Spreadsheets", FileFamilyClassifier.PreferredDocumentCategory("sheet.xlsx"));
        Assert.Equal("Configs", FileFamilyClassifier.PreferredDocumentCategory("app.ini"));
    }
}
