import assert from "node:assert/strict";
import test from "node:test";
import {
  acceptedCount,
  constraintLabel,
  currentWorkflowStep,
  rememberRoot,
  skippedReasonLabel,
} from "./workflow.ts";
import type { ProposalRevision } from "./types";

test("rememberRoot prepends and caps at eight", () => {
  const first = rememberRoot([], "/a");
  assert.deepEqual(first, ["/a"]);
  const next = rememberRoot(["/a", "/b"], "/c");
  assert.deepEqual(next, ["/c", "/a", "/b"]);
  const many = rememberRoot(
    ["/1", "/2", "/3", "/4", "/5", "/6", "/7", "/8"],
    "/9",
  );
  assert.equal(many.length, 8);
  assert.equal(many[0], "/9");
  assert.ok(!many.includes("/8"));
});

test("constraintLabel hides tagged JSON", () => {
  assert.equal(
    constraintLabel({
      kind: "protected",
      reason: "Rust projects depend on Cargo metadata.",
    }),
    "Protected · Rust projects depend on Cargo metadata.",
  );
  assert.equal(constraintLabel({ kind: "move_together" }), "Keep together");
  assert.equal(
    constraintLabel({ kind: "preserve_layout", root: "album" }),
    "Move as a unit · album",
  );
  assert.equal(constraintLabel({ kind: "soft" }), "Suggestion");
});

test("skippedReasonLabel prefers rule and message", () => {
  assert.equal(
    skippedReasonLabel({ path: ".git", reason: { reason: "hidden" } }),
    "hidden",
  );
  assert.equal(
    skippedReasonLabel({
      path: "app/src",
      reason: { reason: "protected_project", rule_id: "rust" },
    }),
    "protected project (rust)",
  );
});

test("currentWorkflowStep follows scan → review → resolve → preview → apply", () => {
  const revision = { id: "r", session: "s", summary: "", placements: {} };
  const snapshot = {
    session: "s",
    root: "/tmp",
    entries: [],
    skipped: [],
    projects: [],
    bundles: [],
    relationships: [],
    evidence: [],
  };
  assert.equal(
    currentWorkflowStep({
      snapshot: null,
      revision: null,
      plan: null,
      issues: [],
      journal: null,
    }),
    "scan",
  );
  assert.equal(
    currentWorkflowStep({
      snapshot,
      revision,
      plan: null,
      issues: [],
      journal: null,
    }),
    "review",
  );
  assert.equal(
    currentWorkflowStep({
      snapshot,
      revision,
      plan: null,
      issues: [{ severity: "error", code: "nothing_accepted", message: "x", assets: [] }],
      journal: null,
    }),
    "resolve",
  );
  assert.equal(
    currentWorkflowStep({
      snapshot,
      revision,
      plan: { id: "p", operations: [] },
      issues: [],
      journal: null,
    }),
    "preview",
  );
  assert.equal(
    currentWorkflowStep({
      snapshot,
      revision,
      plan: { id: "p", operations: [] },
      issues: [],
      journal: { id: "j", status: "completed", dry_run: true, entries: [] },
    }),
    "preview",
  );
  assert.equal(
    currentWorkflowStep({
      snapshot,
      revision,
      plan: { id: "p", operations: [] },
      issues: [],
      journal: { id: "j", status: "completed", dry_run: false, entries: [] },
    }),
    "apply",
  );
});

test("acceptedCount ignores proposed placements", () => {
  const revision: ProposalRevision = {
    id: "r",
    session: "s",
    summary: "",
    placements: {
      a: {
        asset: "a",
        destination: "Documents/a.txt",
        origin: { origin: "heuristic" },
        review: "accepted",
      },
      b: {
        asset: "b",
        destination: "Pictures/b.jpg",
        origin: { origin: "heuristic" },
        review: "proposed",
      },
    },
  };
  assert.equal(acceptedCount(revision), 1);
});
