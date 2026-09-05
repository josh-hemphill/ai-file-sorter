using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Projects;

public sealed record ProjectRule(
    string Id,
    string Name,
    IReadOnlyList<string> RequiredPaths,
    IReadOnlyList<IReadOnlyList<string>> AnyPathGroups,
    IReadOnlyList<string> RootEntrySuffixesAny,
    ProjectStrength Strength,
    string Reason);

/// <summary>Detects project roots that should stay together instead of being sorted file-by-file.</summary>
public sealed class ProjectDetector
{
    private readonly IReadOnlyList<ProjectRule> _rules;

    public ProjectDetector()
        : this(BuiltInRules)
    {
    }

    public ProjectDetector(IReadOnlyList<ProjectRule> rules)
    {
        _rules = rules;
    }

    public static IReadOnlyList<ProjectRule> BuiltInRules { get; } =
    [
        new("unity", "Unity project", ["Assets", "ProjectSettings/ProjectVersion.txt"], [], [], ProjectStrength.Strong,
            "Unity relies on project-relative assets and .meta GUID mappings."),
        new("unreal", "Unreal Engine project", ["Config", "Content"], [], [".uproject"], ProjectStrength.Strong,
            "Unreal projects depend on Content, Config, and .uproject-relative paths."),
        new("godot", "Godot project", ["project.godot"], [], [], ProjectStrength.Strong,
            "Godot projects depend on project.godot and resource-relative paths."),
        new("git", "Git repository", [".git"], [], [], ProjectStrength.Strong,
            "Source repositories depend on stable relative paths tracked by version control."),
        new("node", "Node.js project", ["package.json"], [["package-lock.json", "pnpm-lock.yaml", "yarn.lock", "node_modules", "src"]], [], ProjectStrength.Strong,
            "JavaScript projects depend on package metadata, imports, and build scripts."),
        new("python", "Python project", ["pyproject.toml"], [], [], ProjectStrength.Strong,
            "Python projects depend on package metadata and source-relative imports."),
        new("rust", "Rust project", ["Cargo.toml"], [], [], ProjectStrength.Strong,
            "Rust projects depend on Cargo metadata and source-relative module paths."),
        new("go", "Go module", ["go.mod"], [], [], ProjectStrength.Strong,
            "Go modules depend on module-root-relative package layout."),
        new("gradle", "Gradle project", [], [["settings.gradle", "settings.gradle.kts"], ["build.gradle", "build.gradle.kts", "gradlew"]], [], ProjectStrength.Strong,
            "Gradle projects depend on build files and source-relative layouts."),
        new("dotnet", ".NET project", [], [], [".sln", ".csproj", ".fsproj", ".vbproj"], ProjectStrength.Strong,
            ".NET projects depend on solution and project-relative paths."),
        new("xcode", "Xcode project", [], [], [".xcodeproj", ".xcworkspace"], ProjectStrength.Strong,
            "Xcode projects depend on bundle metadata and project-relative paths."),
        new("blender", "Blender project", [], [["assets", "textures", "materials", "renders", "render", "cache", "blendcache"]], [".blend"], ProjectStrength.Strong,
            "Blender project folders often depend on sibling asset and cache paths."),
        new("blender-file", "Blender scene folder", [], [], [".blend"], ProjectStrength.Weak,
            "A .blend file alone is a weak signal; no automatic scan skip is applied.")
    ];

    public ProjectMatch? Detect(string directory)
    {
        if (!Directory.Exists(directory))
        {
            return null;
        }

        foreach (var rule in _rules)
        {
            if (!RuleMatches(directory, rule))
            {
                continue;
            }

            return new ProjectMatch(Path.GetFullPath(directory), rule.Id, rule.Name, rule.Strength, rule.Reason);
        }

        return null;
    }

    private static bool RuleMatches(string root, ProjectRule rule)
    {
        if (rule.RequiredPaths.Count == 0 &&
            rule.AnyPathGroups.Count == 0 &&
            rule.RootEntrySuffixesAny.Count == 0)
        {
            return false;
        }

        return AllRequiredExist(root, rule.RequiredPaths) &&
               AllGroupsMatch(root, rule.AnyPathGroups) &&
               HasRootEntryWithSuffix(root, rule.RootEntrySuffixesAny);
    }

    private static bool AllRequiredExist(string root, IReadOnlyList<string> paths)
    {
        foreach (var relative in paths)
        {
            if (!PathExists(Path.Combine(root, relative)))
            {
                return false;
            }
        }

        return true;
    }

    private static bool AllGroupsMatch(string root, IReadOnlyList<IReadOnlyList<string>> groups)
    {
        foreach (var group in groups)
        {
            var matched = false;
            foreach (var relative in group)
            {
                if (PathExists(Path.Combine(root, relative)))
                {
                    matched = true;
                    break;
                }
            }

            if (!matched)
            {
                return false;
            }
        }

        return true;
    }

    private static bool HasRootEntryWithSuffix(string root, IReadOnlyList<string> suffixes)
    {
        if (suffixes.Count == 0)
        {
            return true;
        }

        try
        {
            foreach (var entry in Directory.EnumerateFileSystemEntries(root))
            {
                var name = Path.GetFileName(entry).ToLowerInvariant();
                foreach (var suffix in suffixes)
                {
                    if (name.EndsWith(suffix.ToLowerInvariant(), StringComparison.Ordinal))
                    {
                        return true;
                    }
                }
            }
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or IOException)
        {
            return false;
        }

        return false;
    }

    private static bool PathExists(string path)
    {
        try
        {
            return File.Exists(path) || Directory.Exists(path);
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or IOException)
        {
            return false;
        }
    }
}
