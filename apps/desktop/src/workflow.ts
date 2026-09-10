import type {
  ApplyJournal,
  Bundle,
  BundleConstraint,
  CenterView,
  ObservedEntry,
  OperationPlan,
  PlanIssue,
  PlannedOperation,
  ProposalRevision,
  SkippedEntry,
  WorkflowStep,
  WorkspaceSnapshot,
} from "./types";

export const RECENT_ROOTS_KEY = "aifs.recentRoots";
export const RECENT_ROOTS_LIMIT = 8;

export const CENTER_TABS: { id: CenterView; label: string }[] = [
  { id: "structure", label: "Structure" },
  { id: "items", label: "Items" },
  { id: "relationships", label: "Relationships" },
  { id: "activity", label: "Activity" },
];

export const WORKFLOW_STEPS: { id: WorkflowStep; label: string }[] = [
  { id: "scan", label: "Scan" },
  { id: "review", label: "Review" },
  { id: "resolve", label: "Resolve" },
  { id: "preview", label: "Preview" },
  { id: "apply", label: "Apply" },
];

export const INTENT_PRESETS = [
  { id: "inbox" as const, label: "Tidy inbox", hint: "Broad folders, protect projects" },
  { id: "archive" as const, label: "Build archive", hint: "Refined folders, date prefixes" },
  { id: "media" as const, label: "Media library", hint: "Artist/album names from tags" },
  { id: "custom" as const, label: "Custom", hint: "Use the options in Settings" },
];

/** Loads persisted source folders; ignores invalid storage. */
export function loadRecentRoots(): string[] {
  try {
    const raw = globalThis.localStorage?.getItem(RECENT_ROOTS_KEY);
    if (!raw) {
      return [];
    }
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      return [];
    }
    return parsed.filter((item): item is string => typeof item === "string" && item.length > 0);
  } catch {
    return [];
  }
}

/** Remembers a scanned root, newest first. */
export function rememberRoot(existing: string[], path: string): string[] {
  return [path, ...existing.filter((item) => item !== path)].slice(0, RECENT_ROOTS_LIMIT);
}

/** Writes recent roots to localStorage. */
export function persistRecentRoots(paths: string[]): void {
  try {
    globalThis.localStorage?.setItem(RECENT_ROOTS_KEY, JSON.stringify(paths));
  } catch {
    // Private mode or missing WebView storage should not break scanning.
  }
}

const PLACEHOLDER_PROGRESS_MESSAGE = "working";

/** Keeps the last file path when the engine heartbeats with a generic `working` line. */
export function retainProgressMessage(previous: string | undefined, incoming: string): string {
  const next = incoming.trim();
  if (next === "" || next.toLowerCase() === PLACEHOLDER_PROGRESS_MESSAGE) {
    return previous?.trim() ? previous : incoming;
  }
  return incoming;
}

export interface WorkflowState {
  snapshot: WorkspaceSnapshot | null;
  revision: ProposalRevision | null;
  plan: OperationPlan | null;
  issues: PlanIssue[];
  journal: ApplyJournal | null;
}

/** Derives the footer stepper from the current proposal/plan/journal. */
export function currentWorkflowStep(state: WorkflowState): WorkflowStep {
  if (!state.snapshot || !state.revision) {
    return "scan";
  }
  const hasErrors = state.issues.some((issue) => issue.severity === "error");
  if (hasErrors) {
    return "resolve";
  }
  if (
    journalBelongsToPlan(state.journal, state.plan?.id) &&
    state.journal &&
    !state.journal.dry_run &&
    state.journal.status !== "undone"
  ) {
    return "apply";
  }
  if (state.plan) {
    return "preview";
  }
  return "review";
}

/** True when the journal was produced for this plan. */
export function journalBelongsToPlan(
  journal: ApplyJournal | null,
  planId: string | undefined,
): boolean {
  return Boolean(journal?.plan && planId && journal.plan === planId);
}

/** True when this plan already ran a mutating apply. */
export function planAlreadyApplied(
  journal: ApplyJournal | null,
  planId: string | undefined,
): boolean {
  return (
    journalBelongsToPlan(journal, planId) &&
    Boolean(
      journal &&
        !journal.dry_run &&
        (journal.status === "completed" || journal.status === "failed"),
    )
  );
}

/** Human label for a bundle constraint (engine tagged JSON, not the raw object). */
export function constraintLabel(constraint: BundleConstraint | string): string {
  if (typeof constraint === "string") {
    return constraint.replace(/_/g, " ");
  }
  switch (constraint.kind) {
    case "protected":
      return `Protected · ${constraint.reason}`;
    case "move_together":
      return "Keep together";
    case "preserve_layout":
      return `Move as a unit · ${constraint.root}`;
    case "soft":
      return "Suggestion";
    default:
      return "Related";
  }
}

/** Human skip reason for the scan summary. */
export function skippedReasonLabel(entry: SkippedEntry): string {
  const reason = entry.reason.reason.replace(/_/g, " ");
  if (entry.reason.rule_id) {
    return `${reason} (${entry.reason.rule_id})`;
  }
  if (entry.reason.message) {
    return `${reason}: ${entry.reason.message}`;
  }
  return reason;
}

/** Counts accepted placements in a revision. */
export function acceptedCount(revision: ProposalRevision | null): number {
  if (!revision) {
    return 0;
  }
  return Object.values(revision.placements).filter((placement) => placement.review === "accepted")
    .length;
}

export type ItemRow =
  | { type: "file"; entry: ObservedEntry }
  | { type: "group"; bundle: Bundle; members: ObservedEntry[] };

function isHardConstraint(constraint: Bundle["constraint"]): boolean {
  if (typeof constraint === "string") {
    return constraint !== "soft";
  }
  return constraint.kind !== "soft";
}

/** Groups hard-bundle members into single review rows. */
export function itemRows(
  files: ObservedEntry[],
  snapshot: WorkspaceSnapshot | null,
): ItemRow[] {
  if (!snapshot) {
    return files.map((entry) => ({ type: "file", entry }));
  }
  const used = new Set<string>();
  const rows: ItemRow[] = [];
  for (const entry of files) {
    if (used.has(entry.id)) {
      continue;
    }
    const bundle = snapshot.bundles.find(
      (candidate) =>
        isHardConstraint(candidate.constraint) &&
        candidate.members.includes(entry.id) &&
        candidate.members.length > 1,
    );
    if (!bundle) {
      used.add(entry.id);
      rows.push({ type: "file", entry });
      continue;
    }
    const members = snapshot.entries.filter(
      (file) => file.kind === "file" && bundle.members.includes(file.id),
    );
    for (const member of members) {
      used.add(member.id);
    }
    rows.push({ type: "group", bundle, members });
  }
  return rows;
}

/** Human label for a directory role chip. */
export function roleKindLabel(kind: string): string {
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

export type PreviewKind = "create" | "move" | "remove";

export interface PreviewRow {
  seq: number;
  kind: PreviewKind;
  label: string;
  from?: string;
  to?: string;
  state?: string;
  detail?: string;
}

export interface PlanCounts {
  moves: number;
  creates: number;
  removes: number;
}

function journalStateBySeq(
  journal: ApplyJournal | null,
  planId: string | undefined,
): Map<number, { state: string; message?: string; reason?: string }> {
  const map = new Map<number, { state: string; message?: string; reason?: string }>();
  if (!journal || !journalBelongsToPlan(journal, planId)) {
    return map;
  }
  for (const entry of journal.entries) {
    map.set(entry.seq, entry.state);
  }
  return map;
}

function previewKind(operation: PlannedOperation): PreviewKind {
  switch (operation.op) {
    case "create_directory":
      return "create";
    case "remove_empty_directory":
      return "remove";
    default:
      return "move";
  }
}

function previewLabel(operation: PlannedOperation): string {
  switch (operation.op) {
    case "create_directory":
      return `Create ${operation.path}`;
    case "remove_empty_directory":
      return `Remove empty ${operation.path}`;
    case "move":
      return `${operation.from} → ${operation.to}`;
    default:
      return "Change";
  }
}

/** Counts plan operations by kind. */
export function planCounts(plan: OperationPlan | null): PlanCounts {
  const counts: PlanCounts = { moves: 0, creates: 0, removes: 0 };
  if (!plan) {
    return counts;
  }
  for (const planned of plan.operations) {
    switch (planned.operation.op) {
      case "create_directory":
        counts.creates += 1;
        break;
      case "remove_empty_directory":
        counts.removes += 1;
        break;
      default:
        counts.moves += 1;
        break;
    }
  }
  return counts;
}

/** Builds from→to preview rows, overlaying dry-run or apply journal states. */
export function previewRows(
  plan: OperationPlan | null,
  journal: ApplyJournal | null,
): PreviewRow[] {
  const states = journalStateBySeq(journal, plan?.id);
  const operations = plan?.operations ?? [];
  if (operations.length > 0) {
    return operations.map((planned) => {
      const state = states.get(planned.seq);
      return {
        seq: planned.seq,
        kind: previewKind(planned.operation),
        label: previewLabel(planned.operation),
        from: planned.operation.op === "move" ? planned.operation.from : undefined,
        to:
          planned.operation.op === "move"
            ? planned.operation.to
            : planned.operation.op === "create_directory" ||
                planned.operation.op === "remove_empty_directory"
              ? planned.operation.path
              : undefined,
        state: state?.state,
        detail: state?.message ?? state?.reason,
      };
    });
  }
  if (!journal) {
    return [];
  }
  if (plan && !journalBelongsToPlan(journal, plan.id)) {
    return [];
  }
  return journal.entries.map((entry) => {
    const operation = entry.operation;
    if (!operation) {
      return {
        seq: entry.seq,
        kind: "move" as const,
        label: `Operation ${entry.seq}`,
        state: entry.state.state,
        detail: entry.state.message ?? entry.state.reason,
      };
    }
    return {
      seq: entry.seq,
      kind: previewKind(operation),
      label: previewLabel(operation),
      from: operation.op === "move" ? operation.from : undefined,
      to:
        operation.op === "move"
          ? operation.to
          : operation.op === "create_directory" || operation.op === "remove_empty_directory"
            ? operation.path
            : undefined,
      state: entry.state.state,
      detail: entry.state.message ?? entry.state.reason,
    };
  });
}

/** Actionable copy for a plan issue code. */
export function issueLabel(issue: PlanIssue): string {
  switch (issue.code) {
    case "nothing_accepted":
      return "Approve at least one proposed change before Validate. Nothing is moved until you Apply.";
    case "bundle_split":
      return `Keep this bundle together: ${issue.message}`;
    case "destination_occupied":
      return `A file already exists at that destination: ${issue.message}`;
    case "destination_collision":
      return `Two files would land on the same path: ${issue.message}`;
    case "protected_member":
      return `This file is inside a protected project: ${issue.message}`;
    default:
      return issue.message;
  }
}

/** One-line apply confirmation summary. */
export function applyConfirmCopy(root: string, counts: PlanCounts): string {
  const parts = [
    `${counts.moves} move${counts.moves === 1 ? "" : "s"}`,
    `${counts.creates} folder${counts.creates === 1 ? "" : "s"} created`,
    `${counts.removes} empty folder${counts.removes === 1 ? "" : "s"} removed`,
  ];
  return `${root} · ${parts.join(" · ")}`;
}
