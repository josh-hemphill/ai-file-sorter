import assert from "node:assert/strict";
import test from "node:test";
import {
  destinationTree,
  sessionRootProjectBanner,
  sourceTree,
  type SourceTreeNode,
  type TreeNode,
} from "./tree.ts";
import type {
  Bundle,
  DirectoryRoleMatch,
  ObservedEntry,
  ProjectMatch,
  ProposalRevision,
  WorkspaceSnapshot,
} from "./types";

function revisionWith(destination: string): ProposalRevision {
  return {
    id: "r",
    session: "s",
    summary: "t",
    placements: {
      a: {
        asset: "a",
        destination,
        origin: { origin: "heuristic" },
        review: "proposed",
      },
    },
  };
}

function countLeaves(node: TreeNode): number {
  if (node.children.length === 0) {
    return node.fileCount;
  }
  return node.children.reduce((sum, child) => sum + countLeaves(child), 0);
}

function fileEntry(id: string, path: string): ObservedEntry {
  return { id, path, kind: "file", family: "document", identity: { size: 1 } };
}

function dirEntry(id: string, path: string): ObservedEntry {
  return { id, path, kind: "directory", family: "generic", identity: { size: 0 } };
}

function snapshotWith(
  entries: ObservedEntry[],
  extra: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
  return {
    session: "s",
    root: "/tmp/Inbox",
    entries,
    skipped: [],
    projects: [],
    bundles: [],
    relationships: [],
    evidence: [],
    directory_roles: [],
    ...extra,
  };
}

function findNode(node: SourceTreeNode, path: string): SourceTreeNode | undefined {
  if (node.path === path) {
    return node;
  }
  for (const child of node.children) {
    const found = findNode(child, path);
    if (found) {
      return found;
    }
  }
  return undefined;
}

function chipLabels(node: SourceTreeNode | undefined): string[] {
  return node?.chips.map((chip) => chip.label) ?? [];
}

test("destinationTree counts files under folder segments", () => {
  const tree = destinationTree(revisionWith("Documents/Reports/q1.txt"));
  assert.equal(tree.children.length, 1);
  assert.equal(tree.children[0]?.name, "Documents");
  assert.equal(countLeaves(tree), 1);
});

test("sourceTree nests files under directory segments", () => {
  const tree = sourceTree(
    snapshotWith([
      dirEntry("d1", "Music"),
      dirEntry("d2", "Music/Artist"),
      fileEntry("f1", "Music/Artist/track.mp3"),
      fileEntry("f2", "readme.txt"),
    ]),
  );
  assert.equal(tree.name, "Inbox");
  assert.equal(tree.path, ".");
  assert.equal(tree.fileCount, 2);
  const music = findNode(tree, "Music");
  assert.equal(music?.kind, "directory");
  assert.equal(music?.fileCount, 1);
  const track = findNode(tree, "Music/Artist/track.mp3");
  assert.equal(track?.kind, "file");
  assert.equal(track?.assetId, "f1");
  const readme = tree.children.find((child) => child.path === "readme.txt");
  assert.equal(readme?.kind, "file");
});

test("sourceTree directories sort before files", () => {
  const tree = sourceTree(
    snapshotWith([fileEntry("f1", "zeta.txt"), dirEntry("d1", "Alpha")]),
  );
  assert.deepEqual(
    tree.children.map((child) => child.name),
    ["Alpha", "zeta.txt"],
  );
});

test("sourceTree puts role chips on matching directories", () => {
  const roles: DirectoryRoleMatch[] = [
    { root: "Music", kind: "library", reason: "artist/album layout" },
    { root: "Downloads", kind: "broad_inbox", reason: "mixed dump" },
  ];
  const tree = sourceTree(
    snapshotWith(
      [
        dirEntry("d1", "Music"),
        dirEntry("d2", "Downloads"),
        fileEntry("f1", "Music/a.mp3"),
        fileEntry("f2", "Downloads/a.txt"),
      ],
      { directory_roles: roles },
    ),
  );
  assert.deepEqual(chipLabels(findNode(tree, "Music")), ["Library"]);
  assert.deepEqual(chipLabels(findNode(tree, "Downloads")), ["Broad folder"]);
  assert.deepEqual(chipLabels(tree), []);
});

test("sourceTree chips nested strong projects as Protected", () => {
  const projects: ProjectMatch[] = [
    {
      root: ".",
      rule_id: "git",
      name: "Git repository",
      strength: "strong",
      reason: "stable relative paths",
    },
    {
      root: "rust-app",
      rule_id: "rust",
      name: "Rust project",
      strength: "strong",
      reason: "Cargo metadata",
    },
    {
      root: "maybe",
      rule_id: "git",
      name: "Git repository",
      strength: "weak",
      reason: "dot-git only",
    },
  ];
  const tree = sourceTree(
    snapshotWith(
      [
        dirEntry("d1", "rust-app"),
        dirEntry("d2", "maybe"),
        fileEntry("f1", "readme.txt"),
      ],
      { projects },
    ),
  );
  assert.deepEqual(chipLabels(findNode(tree, "rust-app")), ["Protected"]);
  assert.deepEqual(chipLabels(findNode(tree, "maybe")), []);
  assert.ok(!chipLabels(tree).includes("Protected"));
  assert.ok(!chipLabels(findNode(tree, "readme.txt")).includes("Protected"));
});

test("sessionRootProjectBanner explains a strong project at the scan root", () => {
  const projects: ProjectMatch[] = [
    {
      root: ".",
      rule_id: "git",
      name: "Git repository",
      strength: "strong",
      reason: "stable relative paths",
    },
  ];
  const banner = sessionRootProjectBanner(snapshotWith([], { projects }));
  assert.equal(
    banner,
    "Git repository detected at this folder. Files inside can still be organised; nested projects stay protected.",
  );
  assert.equal(
    sessionRootProjectBanner(
      snapshotWith([], {
        projects: [{ ...projects[0]!, strength: "weak" }],
      }),
    ),
    null,
  );
  assert.equal(sessionRootProjectBanner(null), null);
});

test("sourceTree chips PreserveLayout roots as move-as-a-unit", () => {
  const bundles: Bundle[] = [
    {
      id: "b1",
      kind: "directory_role",
      constraint: { kind: "preserve_layout", root: "Ada" },
      members: ["f1", "f2"],
    },
  ];
  const tree = sourceTree(
    snapshotWith(
      [
        dirEntry("d1", "Ada"),
        fileEntry("f1", "Ada/notes.md"),
        fileEntry("f2", "Ada/todo.md"),
      ],
      { bundles },
    ),
  );
  assert.deepEqual(chipLabels(findNode(tree, "Ada")), ["Move as a unit"]);
});

test("sourceTree chips a session-root directory role on the scan folder", () => {
  const tree = sourceTree(
    snapshotWith([fileEntry("f1", "loose.txt")], {
      directory_roles: [{ root: ".", kind: "broad_inbox", reason: "flat dump" }],
    }),
  );
  assert.deepEqual(chipLabels(tree), ["Broad folder"]);
});

test("sourceTree still shows a nested project folder with no interior files", () => {
  const projects: ProjectMatch[] = [
    {
      root: "Game",
      rule_id: "unity",
      name: "Unity project",
      strength: "strong",
      reason: "Assets plus ProjectSettings",
    },
  ];
  const tree = sourceTree(snapshotWith([dirEntry("d1", "Game")], { projects }));
  const game = findNode(tree, "Game");
  assert.equal(game?.kind, "directory");
  assert.equal(game?.fileCount, 0);
  assert.deepEqual(chipLabels(game), ["Protected"]);
});
