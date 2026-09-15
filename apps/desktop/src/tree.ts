import type {
  Bundle,
  DirectoryRoleMatch,
  ProjectMatch,
  ProposalRevision,
  WorkspaceSnapshot,
} from "./types";

/** Session-root sentinel used by the engine (`RelativePath::session_root`). */
export const SESSION_ROOT_PATH = ".";

export interface TreeNode {
  name: string;
  path: string;
  children: TreeNode[];
  fileCount: number;
}

export interface SourceChip {
  kind: string;
  label: string;
  title?: string;
}

export interface SourceTreeNode {
  name: string;
  path: string;
  kind: "file" | "directory";
  assetId?: string;
  fileCount: number;
  children: SourceTreeNode[];
  chips: SourceChip[];
}

/** Builds a destination folder tree from revision placements. */
export function destinationTree(revision: ProposalRevision | null): TreeNode {
  const root: TreeNode = { name: "/", path: "", children: [], fileCount: 0 };
  if (!revision) {
    return root;
  }
  for (const placement of Object.values(revision.placements)) {
    if (placement.review === "rejected") {
      continue;
    }
    const parts = placement.destination.split("/").filter(Boolean);
    if (parts.length === 0) {
      continue;
    }
    parts.pop();
    let cursor = root;
    let prefix = "";
    for (const part of parts) {
      prefix = prefix ? `${prefix}/${part}` : part;
      let child = cursor.children.find((node) => node.name === part);
      if (!child) {
        child = { name: part, path: prefix, children: [], fileCount: 0 };
        cursor.children.push(child);
      }
      cursor = child;
    }
    cursor.fileCount += 1;
  }
  sortTree(root);
  return root;
}

/** Builds a source folder tree from snapshot entries, with role and unit chips. */
export function sourceTree(snapshot: WorkspaceSnapshot | null): SourceTreeNode {
  const sessionName = snapshot ? sessionFolderName(snapshot.root) : "/";
  const nodes = new Map<string, SourceTreeNode>();

  const nodeAt = (path: string): SourceTreeNode => {
    const key = normalizeTreePath(path);
    const existing = nodes.get(key);
    if (existing) {
      return existing;
    }
    const node: SourceTreeNode = {
      name: displayName(key, sessionName),
      path: key,
      kind: "directory",
      fileCount: 0,
      children: [],
      chips: [],
    };
    nodes.set(key, node);
    if (key !== SESSION_ROOT_PATH) {
      nodeAt(parentPath(key)).children.push(node);
    }
    return node;
  };

  const root = nodeAt(SESSION_ROOT_PATH);
  if (!snapshot) {
    return root;
  }

  for (const entry of snapshot.entries) {
    if (entry.kind === "directory") {
      nodeAt(entry.path);
    }
  }
  for (const entry of snapshot.entries) {
    if (entry.kind === "directory") {
      continue;
    }
    const file = nodeAt(entry.path);
    file.kind = "file";
    file.assetId = entry.id;
  }
  for (const path of chipAnchorPaths(snapshot)) {
    nodeAt(path);
  }

  attachChips(nodes, snapshot);
  countFiles(root);
  sortSourceTree(root);
  return root;
}

/** Banner when a strong project sits on the scanned folder itself. */
export function sessionRootProjectBanner(
  snapshot: WorkspaceSnapshot | null,
): string | null {
  if (!snapshot) {
    return null;
  }
  const project = snapshot.projects.find(
    (item) => isSessionRootPath(item.root) && item.strength === "strong",
  );
  if (!project) {
    return null;
  }
  return `${project.name} detected at this folder. Files inside can still be organised; nested projects stay protected.`;
}

function attachChips(
  nodes: Map<string, SourceTreeNode>,
  snapshot: WorkspaceSnapshot,
): void {
  for (const project of snapshot.projects) {
    if (project.strength !== "strong" || isSessionRootPath(project.root)) {
      continue;
    }
    addChip(nodes.get(normalizeTreePath(project.root)), protectedChip(project));
  }
  for (const bundle of snapshot.bundles) {
    const root = preserveLayoutRoot(bundle);
    if (!root || isSessionRootPath(root)) {
      continue;
    }
    addChip(nodes.get(normalizeTreePath(root)), {
      kind: "preserve_layout",
      label: "Move as a unit",
      title: root,
    });
  }
  for (const role of snapshot.directory_roles ?? []) {
    addChip(nodes.get(normalizeTreePath(role.root)), roleChip(role));
  }
}

function chipAnchorPaths(snapshot: WorkspaceSnapshot): string[] {
  const paths: string[] = [];
  for (const project of snapshot.projects) {
    paths.push(project.root);
  }
  for (const role of snapshot.directory_roles ?? []) {
    paths.push(role.root);
  }
  for (const bundle of snapshot.bundles) {
    const root = preserveLayoutRoot(bundle);
    if (root) {
      paths.push(root);
    }
  }
  return paths;
}

function preserveLayoutRoot(bundle: Bundle): string | null {
  if (typeof bundle.constraint !== "object") {
    return null;
  }
  if (bundle.constraint.kind !== "preserve_layout") {
    return null;
  }
  return bundle.constraint.root;
}

function protectedChip(project: ProjectMatch): SourceChip {
  return {
    kind: "protected",
    label: "Protected",
    title: project.reason,
  };
}

function roleChip(role: DirectoryRoleMatch): SourceChip {
  return {
    kind: role.kind,
    label: roleChipLabel(role.kind),
    title: role.reason,
  };
}

/** Keep chip copy aligned with `roleKindLabel` in workflow.ts. */
function roleChipLabel(kind: string): string {
  switch (kind) {
    case "library":
      return "Library";
    case "broad_inbox":
      return "Broad folder";
    case "weak_archive":
      return "Archive context";
    case "mixed":
      return "Mixed";
    default:
      return kind.replace(/_/g, " ");
  }
}

function addChip(node: SourceTreeNode | undefined, chip: SourceChip): void {
  if (!node || node.kind !== "directory") {
    return;
  }
  if (node.chips.some((existing) => existing.kind === chip.kind)) {
    return;
  }
  node.chips.push(chip);
}

function countFiles(node: SourceTreeNode): number {
  if (node.kind === "file") {
    node.fileCount = 1;
    return 1;
  }
  let total = 0;
  for (const child of node.children) {
    total += countFiles(child);
  }
  node.fileCount = total;
  return total;
}

function sortSourceTree(node: SourceTreeNode): void {
  node.children.sort((a, b) => {
    if (a.kind !== b.kind) {
      return a.kind === "directory" ? -1 : 1;
    }
    return a.name.localeCompare(b.name);
  });
  for (const child of node.children) {
    sortSourceTree(child);
  }
}

function sortTree(node: TreeNode): void {
  node.children.sort((a, b) => a.name.localeCompare(b.name));
  for (const child of node.children) {
    sortTree(child);
  }
}

function isSessionRootPath(path: string): boolean {
  return path === SESSION_ROOT_PATH || path === "";
}

function normalizeTreePath(path: string): string {
  return isSessionRootPath(path) ? SESSION_ROOT_PATH : path;
}

function parentPath(path: string): string {
  if (isSessionRootPath(path)) {
    return SESSION_ROOT_PATH;
  }
  const slash = path.lastIndexOf("/");
  if (slash < 0) {
    return SESSION_ROOT_PATH;
  }
  return path.slice(0, slash);
}

function displayName(path: string, sessionName: string): string {
  if (isSessionRootPath(path)) {
    return sessionName;
  }
  const slash = path.lastIndexOf("/");
  return slash < 0 ? path : path.slice(slash + 1);
}

function sessionFolderName(root: string): string {
  const trimmed = root.replace(/[\\/]+$/, "");
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? "/";
}
