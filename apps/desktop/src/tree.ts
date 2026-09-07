import type { ProposalRevision } from "./types";

export interface TreeNode {
  name: string;
  path: string;
  children: TreeNode[];
  fileCount: number;
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

function sortTree(node: TreeNode): void {
  node.children.sort((a, b) => a.name.localeCompare(b.name));
  for (const child of node.children) {
    sortTree(child);
  }
}
