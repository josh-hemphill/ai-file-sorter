import assert from "node:assert/strict";
import test from "node:test";
import {
  acceptedCount,
  applyConfirmCopy,
  constraintLabel,
  currentWorkflowStep,
  issueLabel,
  itemRows,
  planAlreadyApplied,
  planCounts,
  previewRows,
  rememberRoot,
  retainProgressMessage,
  cancelActionLabel,
  inFlightStatusText,
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

test("retainProgressMessage keeps the last path across working heartbeats", () => {
  const path = "Music/What Is Love?.mp3";
  assert.equal(retainProgressMessage(undefined, path), path);
  assert.equal(retainProgressMessage(path, "working"), path);
  assert.equal(retainProgressMessage(path, "Working"), path);
  assert.equal(retainProgressMessage(path, "  "), path);
  assert.equal(retainProgressMessage(path, "Music/next.mp3"), "Music/next.mp3");
  assert.equal(retainProgressMessage(undefined, "working"), "working");
});

test("cancelActionLabel and inFlightStatusText show cancelling state", () => {
  assert.equal(cancelActionLabel(false), "Cancel");
  assert.equal(cancelActionLabel(true), "Cancelling…");
  assert.equal(inFlightStatusText(false, "Working… scan 3"), "Working… scan 3");
  assert.equal(inFlightStatusText(true, "Working… scan 3"), "Cancelling…");
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
      journal: { id: "j", plan: "p", status: "completed", dry_run: false, entries: [] },
    }),
    "apply",
  );
  assert.equal(
    currentWorkflowStep({
      snapshot,
      revision,
      plan: { id: "p2", operations: [] },
      issues: [],
      journal: { id: "j", plan: "p1", status: "completed", dry_run: false, entries: [] },
    }),
    "preview",
  );
  assert.equal(
    currentWorkflowStep({
      snapshot,
      revision,
      plan: null,
      issues: [],
      journal: { id: "j", status: "completed", dry_run: false, entries: [] },
    }),
    "review",
  );
});

test("planAlreadyApplied requires a matching mutating journal", () => {
  assert.equal(
    planAlreadyApplied(
      { id: "j", plan: "p", status: "completed", dry_run: false, entries: [] },
      "p",
    ),
    true,
  );
  assert.equal(
    planAlreadyApplied(
      { id: "j", plan: "p", status: "completed", dry_run: true, entries: [] },
      "p",
    ),
    false,
  );
  assert.equal(
    planAlreadyApplied(
      { id: "j", plan: "p1", status: "completed", dry_run: false, entries: [] },
      "p2",
    ),
    false,
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

test("itemRows collapses hard bundles", () => {
  const photo = {
    id: "1",
    path: "photo.jpg",
    kind: "file" as const,
    family: "image",
    identity: { size: 1 },
  };
  const xmp = {
    id: "2",
    path: "photo.xmp",
    kind: "file" as const,
    family: "sidecar",
    identity: { size: 1 },
  };
  const note = {
    id: "3",
    path: "readme.txt",
    kind: "file" as const,
    family: "document",
    identity: { size: 1 },
  };
  const rows = itemRows([photo, xmp, note], {
    session: "s",
    root: "/tmp",
    entries: [photo, xmp, note],
    skipped: [],
    projects: [],
    bundles: [
      {
        id: "b",
        kind: "sidecar_group",
        constraint: { kind: "move_together" },
        members: ["1", "2"],
        label: "photo",
      },
    ],
    relationships: [],
    evidence: [],
  });
  assert.equal(rows.length, 2);
  assert.equal(rows[0]?.type, "group");
  assert.equal(rows[1]?.type, "file");
});

test("itemRows keeps hidden hard-bundle members", () => {
  const photo = {
    id: "1",
    path: "photo.jpg",
    kind: "file" as const,
    family: "image",
    identity: { size: 1 },
  };
  const xmp = {
    id: "2",
    path: "photo.xmp",
    kind: "file" as const,
    family: "sidecar",
    identity: { size: 1 },
  };
  const snapshot = {
    session: "s",
    root: "/tmp",
    entries: [photo, xmp],
    skipped: [],
    projects: [],
    bundles: [
      {
        id: "b",
        kind: "sidecar_group",
        constraint: { kind: "move_together" as const },
        members: ["1", "2"],
        label: "photo",
      },
    ],
    relationships: [],
    evidence: [],
  };
  const rows = itemRows([photo], snapshot);
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.type, "group");
  if (rows[0]?.type === "group") {
    assert.equal(rows[0].members.length, 2);
  }
});

test("previewRows maps plan operations to from→to rows", () => {
  const plan = {
    id: "p",
    operations: [
      { seq: 0, operation: { op: "create_directory" as const, path: "Documents" } },
      {
        seq: 1,
        operation: {
          op: "move" as const,
          asset: "a",
          from: "dump/a.txt",
          to: "Documents/a.txt",
        },
      },
      { seq: 2, operation: { op: "remove_empty_directory" as const, path: "dump" } },
    ],
  };
  const rows = previewRows(plan, {
    id: "j",
    plan: "p",
    status: "completed",
    dry_run: true,
    entries: [
      { seq: 0, state: { state: "intended" } },
      { seq: 1, state: { state: "intended" } },
      { seq: 2, state: { state: "intended" } },
    ],
  });
  assert.equal(rows.length, 3);
  assert.equal(rows[1]?.from, "dump/a.txt");
  assert.equal(rows[1]?.to, "Documents/a.txt");
  assert.equal(rows[2]?.kind, "remove");
  assert.equal(rows[0]?.state, "intended");
  assert.deepEqual(planCounts(plan), { moves: 1, creates: 1, removes: 1 });
});

test("previewRows ignores a journal from a different plan", () => {
  const plan = {
    id: "p2",
    operations: [
      { seq: 0, operation: { op: "move" as const, asset: "a", from: "a.txt", to: "Documents/a.txt" } },
    ],
  };
  const rows = previewRows(plan, {
    id: "j",
    plan: "p1",
    status: "completed",
    dry_run: false,
    entries: [{ seq: 0, state: { state: "done" } }],
  });
  assert.equal(rows[0]?.state, undefined);
});

test("previewRows ignores a journal with no plan id", () => {
  const plan = {
    id: "p",
    operations: [
      { seq: 0, operation: { op: "move" as const, asset: "a", from: "a.txt", to: "Documents/a.txt" } },
    ],
  };
  const rows = previewRows(plan, {
    id: "j",
    status: "completed",
    dry_run: false,
    entries: [{ seq: 0, state: { state: "done" } }],
  });
  assert.equal(rows[0]?.state, undefined);
});

test("previewRows ignores a foreign journal when the plan has no operations", () => {
  const plan = { id: "p2", operations: [] };
  const rows = previewRows(plan, {
    id: "j",
    plan: "p1",
    status: "completed",
    dry_run: false,
    entries: [
      {
        seq: 0,
        operation: { op: "move" as const, asset: "a", from: "old.txt", to: "Documents/old.txt" },
        state: { state: "done" },
      },
    ],
  });
  assert.equal(rows.length, 0);
});

test("issueLabel explains approve-before-validate", () => {
  assert.equal(
    issueLabel({
      severity: "error",
      code: "nothing_accepted",
      message: "no placements",
      assets: [],
    }).includes("Approve at least one"),
    true,
  );
});

test("applyConfirmCopy names the source and counts", () => {
  assert.equal(
    applyConfirmCopy("/tmp/inbox", { moves: 1, creates: 1, removes: 1 }),
    "/tmp/inbox · 1 move · 1 folder created · 1 empty folder removed",
  );
});
