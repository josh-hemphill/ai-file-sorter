import type {
  ApplyJournal,
  BundleConstraint,
  CenterView,
  OperationPlan,
  PlanIssue,
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
  if (state.journal && !state.journal.dry_run && state.journal.status !== "undone") {
    return "apply";
  }
  if (state.plan) {
    return "preview";
  }
  return "review";
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
