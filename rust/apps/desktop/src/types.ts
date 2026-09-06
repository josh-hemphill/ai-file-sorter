/** JSON shapes produced by the engine (serde snake_case). */

export type Id = string;

export interface ObservedEntry {
  id: Id;
  path: string;
  kind: "file" | "directory" | "symlink";
  family: string;
  identity: { size: number };
}

export type BundleConstraint =
  | { kind: "soft" }
  | { kind: "move_together" }
  | { kind: "preserve_layout"; root: string }
  | { kind: "protected"; reason: string };

export interface Bundle {
  id: Id;
  kind: string;
  constraint: BundleConstraint | string;
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

export interface DirectoryRoleMatch {
  root: string;
  kind: "library" | "broad_inbox" | "weak_archive" | "mixed";
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
  directory_roles?: DirectoryRoleMatch[];
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

export type PlannedOperation =
  | { op: "create_directory"; path: string }
  | { op: "move"; asset: Id; from: string; to: string; expected?: { size: number } }
  | { op: "remove_empty_directory"; path: string };

export interface OperationPlan {
  id: Id;
  operations: Array<{
    seq: number;
    operation: PlannedOperation;
  }>;
}

export interface ApplyJournal {
  id: Id;
  plan?: Id;
  status: string;
  dry_run: boolean;
  entries: Array<{
    seq: number;
    operation?: PlannedOperation;
    state: { state: string; message?: string; reason?: string };
  }>;
}

export interface ProgressEvent {
  stage: string;
  current: number;
  total: number | null;
  message: string;
}

export interface LogEvent {
  level: string;
  message: string;
}

export type CenterView = "structure" | "items" | "relationships" | "activity";
export type IntentPreset = "inbox" | "archive" | "media" | "custom";
export type WorkflowStep = "scan" | "review" | "resolve" | "preview" | "apply";
export type AppSurface = "workspace" | "settings" | "setup";

export interface ScanOptions {
  recursive: boolean;
  max_depth: number;
  include_hidden: boolean;
  protect_projects: boolean;
  extract_metadata: boolean;
  fingerprint_prefix_bytes: number;
}

export interface CategoryWhitelist {
  main: string[];
  global_subcategories: string[];
  branching: Record<string, string[]>;
}

export interface ProposalPolicy {
  style: "consistent" | "refined";
  use_subfolders: boolean;
  rename_media: boolean;
  rename_images_with_date: boolean;
  pinned_families: string[];
  project_folder: string | null;
  whitelist: CategoryWhitelist;
  category_language: string;
}

export interface AppSettings {
  scan: ScanOptions;
  policy: ProposalPolicy;
  analyze_images: boolean;
  analyze_documents: boolean;
}

export type ModelBackend =
  | { kind: "off" }
  | { kind: "catalog"; catalog_id: string }
  | { kind: "local_gguf"; path: string; mmproj?: string }
  | { kind: "open_ai"; model: string }
  | { kind: "gemini"; model: string }
  | { kind: "custom_endpoint"; base_url: string; model: string };

export interface ModelSlot {
  id: string;
  kind: ModelBackend["kind"];
  catalog_id?: string;
  path?: string;
  mmproj?: string;
  model?: string;
  base_url?: string;
  api_key?: string;
  api_key_set?: boolean;
}

export interface ModelInventory {
  storage_dir: string;
  gpu_preference: string;
  slots: ModelSlot[];
}

export interface ChatReply {
  message: string;
  revision: ProposalRevision | null;
}

export interface ChatLine {
  role: "you" | "assistant";
  text: string;
}
