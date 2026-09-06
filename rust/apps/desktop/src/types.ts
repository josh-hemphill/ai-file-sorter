/** JSON shapes produced by the engine (serde snake_case). */

export type Id = string;

export interface ObservedEntry {
  id: Id;
  path: string;
  kind: "file" | "directory" | "symlink";
  family: string;
  identity: { size: number };
}

export interface Bundle {
  id: Id;
  kind: string;
  constraint: string;
  members: Id[];
  label?: string;
}

export interface Relationship {
  from: Id;
  to: Id;
  kind: string;
  detector: string;
  note?: string;
}

export interface Evidence {
  asset: Id;
  source: { source: string; name?: string; model?: string };
  facts: Record<string, string>;
}

export interface SkippedEntry {
  path: string;
  reason: { reason: string; rule_id?: string; message?: string };
}

export interface ProjectMatch {
  root: string;
  rule_id: string;
  name: string;
  strength: string;
  reason: string;
}

export interface WorkspaceSnapshot {
  session: Id;
  root: string;
  entries: ObservedEntry[];
  skipped: SkippedEntry[];
  projects: ProjectMatch[];
  bundles: Bundle[];
  relationships: Relationship[];
  evidence: Evidence[];
}

export interface Placement {
  asset: Id;
  destination: string;
  rationale?: string;
  origin: { origin: string };
  review: "proposed" | "accepted" | "rejected";
}

export interface ProposalRevision {
  id: Id;
  session: Id;
  summary: string;
  placements: Record<Id, Placement>;
}

export interface PlanIssue {
  severity: "error" | "warning";
  code: string;
  message: string;
  assets: Id[];
}

export interface OperationPlan {
  id: Id;
  operations: Array<{
    seq: number;
    operation:
      | { op: "create_directory"; path: string }
      | { op: "move"; asset: Id; from: string; to: string }
      | { op: "remove_empty_directory"; path: string };
  }>;
}

export interface ApplyJournal {
  id: Id;
  status: string;
  dry_run: boolean;
  entries: Array<{ seq: number; state: { state: string; message?: string; reason?: string } }>;
}

export interface ProgressEvent {
  stage: string;
  current: number;
  total: number | null;
  message: string;
}

export type CenterView = "structure" | "items" | "relationships" | "activity";
export type IntentPreset = "inbox" | "archive" | "media" | "custom";
