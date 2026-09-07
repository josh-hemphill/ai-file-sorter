//! Protected-project rules ported from the Qt `ProtectedProjectDetector`.

use aifs_domain::{ProjectMatch, ProjectStrength, RelativePath};
use std::path::Path;

/// A project rule that matched a directory on disk, before a relative path is attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedProject {
    /// Rule id (e.g. `unity`, `git`).
    pub rule_id: String,
    /// Display name.
    pub name: String,
    /// Strength.
    pub strength: ProjectStrength,
    /// Why moving members independently is unsafe.
    pub reason: String,
}

impl DetectedProject {
    /// Binds the match to a path relative to the session root.
    pub fn into_match(self, root: RelativePath) -> ProjectMatch {
        ProjectMatch {
            root,
            rule_id: self.rule_id,
            name: self.name,
            strength: self.strength,
            reason: self.reason,
        }
    }
}

struct ProjectRule {
    id: &'static str,
    name: &'static str,
    required_paths: &'static [&'static str],
    any_path_groups: &'static [&'static [&'static str]],
    root_entry_suffixes_any: &'static [&'static str],
    strength: ProjectStrength,
    reason: &'static str,
}

const BUILTIN_RULES: &[ProjectRule] = &[
    ProjectRule {
        id: "unity",
        name: "Unity project",
        required_paths: &["Assets", "ProjectSettings/ProjectVersion.txt"],
        any_path_groups: &[],
        root_entry_suffixes_any: &[],
        strength: ProjectStrength::Strong,
        reason: "Unity relies on project-relative assets and .meta GUID mappings.",
    },
    ProjectRule {
        id: "unreal",
        name: "Unreal Engine project",
        required_paths: &["Config", "Content"],
        any_path_groups: &[],
        root_entry_suffixes_any: &[".uproject"],
        strength: ProjectStrength::Strong,
        reason: "Unreal projects depend on Content, Config, and .uproject-relative paths.",
    },
    ProjectRule {
        id: "godot",
        name: "Godot project",
        required_paths: &["project.godot"],
        any_path_groups: &[],
        root_entry_suffixes_any: &[],
        strength: ProjectStrength::Strong,
        reason: "Godot projects depend on project.godot and resource-relative paths.",
    },
    ProjectRule {
        id: "git",
        name: "Git repository",
        required_paths: &[".git"],
        any_path_groups: &[],
        root_entry_suffixes_any: &[],
        strength: ProjectStrength::Strong,
        reason: "Source repositories depend on stable relative paths tracked by version control.",
    },
    ProjectRule {
        id: "node",
        name: "Node.js project",
        required_paths: &["package.json"],
        any_path_groups: &[&[
            "package-lock.json",
            "pnpm-lock.yaml",
            "yarn.lock",
            "node_modules",
            "src",
        ]],
        root_entry_suffixes_any: &[],
        strength: ProjectStrength::Strong,
        reason: "JavaScript projects depend on package metadata, imports, and build scripts.",
    },
    ProjectRule {
        id: "python",
        name: "Python project",
        required_paths: &["pyproject.toml"],
        any_path_groups: &[],
        root_entry_suffixes_any: &[],
        strength: ProjectStrength::Strong,
        reason: "Python projects depend on package metadata and source-relative imports.",
    },
    ProjectRule {
        id: "rust",
        name: "Rust project",
        required_paths: &["Cargo.toml"],
        any_path_groups: &[],
        root_entry_suffixes_any: &[],
        strength: ProjectStrength::Strong,
        reason: "Rust projects depend on Cargo metadata and source-relative module paths.",
    },
    ProjectRule {
        id: "go",
        name: "Go module",
        required_paths: &["go.mod"],
        any_path_groups: &[],
        root_entry_suffixes_any: &[],
        strength: ProjectStrength::Strong,
        reason: "Go modules depend on module-root-relative package layout.",
    },
    ProjectRule {
        id: "gradle",
        name: "Gradle project",
        required_paths: &[],
        any_path_groups: &[
            &["settings.gradle", "settings.gradle.kts"],
            &["build.gradle", "build.gradle.kts", "gradlew"],
        ],
        root_entry_suffixes_any: &[],
        strength: ProjectStrength::Strong,
        reason: "Gradle projects depend on build files and source-relative layouts.",
    },
    ProjectRule {
        id: "dotnet",
        name: ".NET project",
        required_paths: &[],
        any_path_groups: &[],
        root_entry_suffixes_any: &[".sln", ".csproj", ".fsproj", ".vbproj"],
        strength: ProjectStrength::Strong,
        reason: ".NET projects depend on solution and project-relative paths.",
    },
    ProjectRule {
        id: "xcode",
        name: "Xcode project",
        required_paths: &[],
        any_path_groups: &[],
        root_entry_suffixes_any: &[".xcodeproj", ".xcworkspace"],
        strength: ProjectStrength::Strong,
        reason: "Xcode projects depend on bundle metadata and project-relative paths.",
    },
    ProjectRule {
        id: "blender",
        name: "Blender project",
        required_paths: &[],
        any_path_groups: &[&[
            "assets",
            "textures",
            "materials",
            "renders",
            "render",
            "cache",
            "blendcache",
        ]],
        root_entry_suffixes_any: &[".blend"],
        strength: ProjectStrength::Strong,
        reason: "Blender project folders often depend on sibling asset and cache paths.",
    },
    ProjectRule {
        id: "blender-file",
        name: "Blender scene folder",
        required_paths: &[],
        any_path_groups: &[],
        root_entry_suffixes_any: &[".blend"],
        strength: ProjectStrength::Weak,
        reason: "A .blend file alone is a weak signal; no automatic scan skip is applied.",
    },
];

/// Evaluates built-in project rules against `directory`. First match wins.
pub fn detect_project(directory: &Path) -> Option<DetectedProject> {
    if !directory.is_dir() {
        return None;
    }
    for rule in BUILTIN_RULES {
        if rule_matches(directory, rule) {
            return Some(DetectedProject {
                rule_id: rule.id.to_owned(),
                name: rule.name.to_owned(),
                strength: rule.strength,
                reason: rule.reason.to_owned(),
            });
        }
    }
    None
}

/// True when a strong match should stop recursive traversal.
pub fn should_skip_traversal(project: &DetectedProject) -> bool {
    project.strength == ProjectStrength::Strong
}

fn rule_matches(root: &Path, rule: &ProjectRule) -> bool {
    let has_marker = !rule.required_paths.is_empty()
        || !rule.any_path_groups.is_empty()
        || !rule.root_entry_suffixes_any.is_empty();
    if !has_marker {
        return false;
    }
    rule.required_paths
        .iter()
        .all(|marker| root.join(marker).exists())
        && rule
            .any_path_groups
            .iter()
            .all(|group| group.iter().any(|marker| root.join(marker).exists()))
        && has_root_entry_with_suffix(root, rule.root_entry_suffixes_any)
}

fn has_root_entry_with_suffix(root: &Path, suffixes: &[&str]) -> bool {
    if suffixes.is_empty() {
        return true;
    }
    let read_dir = match root.read_dir() {
        Ok(iter) => iter,
        Err(_) => return false,
    };
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if suffixes
            .iter()
            .any(|suffix| name.ends_with(&suffix.to_ascii_lowercase()))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn unity_is_strong_and_git_beats_later_rules() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::create_dir_all(dir.path().join("Assets")).unwrap_or_else(|e| panic!("{e}"));
        fs::create_dir_all(dir.path().join("ProjectSettings")).unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("ProjectSettings/ProjectVersion.txt"), "1")
            .unwrap_or_else(|e| panic!("{e}"));
        let matched = detect_project(dir.path()).unwrap_or_else(|| panic!("expected unity"));
        assert_eq!(matched.rule_id, "unity");
        assert!(should_skip_traversal(&matched));
    }

    #[test]
    fn blender_file_alone_is_weak() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("scene.blend"), []).unwrap_or_else(|e| panic!("{e}"));
        let matched = detect_project(dir.path()).unwrap_or_else(|| panic!("expected blender-file"));
        assert_eq!(matched.rule_id, "blender-file");
        assert!(!should_skip_traversal(&matched));
    }

    #[test]
    fn blender_with_textures_is_strong() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("scene.blend"), []).unwrap_or_else(|e| panic!("{e}"));
        fs::create_dir(dir.path().join("textures")).unwrap_or_else(|e| panic!("{e}"));
        let matched = detect_project(dir.path()).unwrap_or_else(|| panic!("expected blender"));
        assert_eq!(matched.rule_id, "blender");
        assert!(should_skip_traversal(&matched));
    }

    #[test]
    fn node_requires_package_json_and_a_companion_marker() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        fs::write(dir.path().join("package.json"), "{}").unwrap_or_else(|e| panic!("{e}"));
        assert!(detect_project(dir.path()).is_none());
        fs::create_dir(dir.path().join("src")).unwrap_or_else(|e| panic!("{e}"));
        let matched = detect_project(dir.path()).unwrap_or_else(|| panic!("expected node"));
        assert_eq!(matched.rule_id, "node");
    }
}
