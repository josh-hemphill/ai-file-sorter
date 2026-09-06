import assert from "node:assert/strict";
import test from "node:test";
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

test("destinationTree counts files under folder segments", () => {
  const tree = destinationTree(revisionWith("Documents/Reports/q1.txt"));
  assert.equal(tree.children.length, 1);
  assert.equal(tree.children[0]?.name, "Documents");
  assert.equal(countLeaves(tree), 1);
});
