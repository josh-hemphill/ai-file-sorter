import { destinationTree, type TreeNode } from "./tree.ts";
import type { ProposalRevision } from "./types";

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

const tree = destinationTree(
  revisionWith("Documents/Reports/q1.txt"),
);
if (tree.children.length !== 1 || tree.children[0]?.name !== "Documents") {
  throw new Error("expected Documents root folder");
}
if (countLeaves(tree) !== 1) {
  throw new Error("expected one file counted");
}
console.log("tree test ok");
