import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ApplyJournal,
  ChatReply,
  Id,
  LogEvent,
  OperationPlan,
  PlanIssue,
  ProgressEvent,
  ProposalRevision,
  WorkspaceSnapshot,
  AppSettings,
  ModelBackend,
  ModelInventory,
} from "./types";

export async function connectEngine(): Promise<void> {
  await invoke("connect_engine");
}

export async function pickFolder(): Promise<string | null> {
  return invoke("pick_folder");
}

export async function scanRoot(
  root: string,
  preset: string,
): Promise<WorkspaceSnapshot> {
  return invoke("scan_root", { args: { root, preset } });
}

export async function proposeSession(
  session: Id,
  preset: string,
): Promise<ProposalRevision> {
  return invoke("propose_session", { session, preset });
}

export async function patchRevision(
  session: Id,
  baseRevision: Id,
  summary: string,
  patches: unknown[],
): Promise<ProposalRevision> {
  return invoke("patch_revision", {
    session,
    baseRevision,
    summary,
    patches,
  });
}

export async function planRevision(
  session: Id,
  revision: Id,
): Promise<{ plan: OperationPlan | null; issues: PlanIssue[] }> {
  return invoke("plan_revision", { session, revision });
}

export async function applyPlan(
  session: Id,
  plan: Id,
  dryRun: boolean,
): Promise<ApplyJournal> {
  return invoke("apply_plan", { session, plan, dryRun });
}

export async function undoJournal(
  session: Id,
  journal: Id,
): Promise<ApplyJournal> {
  return invoke("undo_journal", { session, journal });
}

export async function chatRevision(
  session: Id,
  revision: Id,
  utterance: string,
): Promise<ChatReply> {
  return invoke("chat_revision", { session, revision, utterance });
}

export function onEngineProgress(
  handler: (event: ProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<ProgressEvent>("engine-progress", (event) => handler(event.payload));
}

export async function onEngineLog(handler: (event: LogEvent) => void): Promise<UnlistenFn> {
  return listen<LogEvent>("engine-log", (event) => handler(event.payload));
}

export async function getSettings(): Promise<AppSettings> {
  return invoke("get_settings");
}

export async function putSettings(settings: AppSettings): Promise<AppSettings> {
  return invoke("put_settings", { settings });
}

export async function getModels(): Promise<ModelInventory> {
  return invoke("get_models");
}

export async function putModels(inventory: ModelInventory): Promise<ModelInventory> {
  return invoke("put_models", { inventory });
}

export async function probeEndpoint(
  backend: ModelBackend,
  apiKey?: string,
): Promise<[boolean, string]> {
  return invoke("probe_endpoint", { args: { ...backend, api_key: apiKey } });
}
