//! Tauri shell: launches `aifs-engine` and forwards typed commands. No scan or
//! mutation happens in this process beyond spawning the engine.

use aifs_domain::{
    ApplyJournal, JournalId, OperationPlan, PlanId, PlanIssue, ProposalRevision, RevisionAuthor,
    RevisionId, RevisionPatch, SessionId, WorkspaceSnapshot,
};
use aifs_engine_client::{EngineClient, discover_engine_binary};
use aifs_protocol::{
    AppSettings, Event, FolderStyle, LogLevel, ModelBackend, ModelInventory, ProposalPolicy,
    ScanOptions,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

struct EngineState {
    client: Mutex<Option<Arc<EngineClient>>>,
}

#[derive(Clone, Serialize)]
struct ProgressPayload {
    stage: String,
    current: u64,
    total: Option<u64>,
    message: String,
}

#[derive(Clone, Serialize)]
struct LogPayload {
    level: String,
    message: String,
}

fn forward_engine_event(app: &AppHandle, event: &Event) {
    match event {
        Event::Progress {
            stage,
            current,
            total,
            message,
        } => {
            let _ = app.emit(
                "engine-progress",
                ProgressPayload {
                    stage: stage.clone(),
                    current: *current,
                    total: *total,
                    message: message.clone(),
                },
            );
        }
        Event::Log { level, message } => {
            let level = match level {
                LogLevel::Debug => "debug",
                LogLevel::Info => "info",
                LogLevel::Warn => "warn",
                LogLevel::Error => "error",
            };
            let _ = app.emit(
                "engine-log",
                LogPayload {
                    level: level.to_owned(),
                    message: message.clone(),
                },
            );
        }
        _ => {}
    }
}

fn clone_client(state: &EngineState) -> Result<Arc<EngineClient>, String> {
    let guard = state
        .client
        .lock()
        .map_err(|_| "engine lock poisoned".to_owned())?;
    guard
        .as_ref()
        .cloned()
        .ok_or_else(|| "engine is not running".to_owned())
}

fn with_client<T>(
    state: &EngineState,
    fun: impl FnOnce(&EngineClient) -> Result<T, String>,
) -> Result<T, String> {
    let client = clone_client(state)?;
    fun(&client)
}

fn ensure_client(state: &EngineState) -> Result<(), String> {
    let mut guard = state
        .client
        .lock()
        .map_err(|_| "engine lock poisoned".to_owned())?;
    if guard.is_some() {
        return Ok(());
    }
    let binary = discover_engine_binary().map_err(|error| error.to_string())?;
    let client =
        EngineClient::connect(binary, "aifs-desktop").map_err(|error| error.to_string())?;
    *guard = Some(Arc::new(client));
    Ok(())
}

#[tauri::command]
fn connect_engine(state: State<EngineState>) -> Result<(), String> {
    ensure_client(&state)
}

#[tauri::command]
fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app.dialog().file().blocking_pick_folder();
    Ok(picked.map(|path| path.to_string()))
}

#[derive(Debug, Deserialize)]
struct ScanArgs {
    root: String,
    preset: String,
}

fn policy_for_preset(preset: &str) -> Option<(ScanOptions, ProposalPolicy)> {
    if preset == "custom" {
        return None;
    }
    let mut scan = ScanOptions::default();
    let mut policy = ProposalPolicy::default();
    match preset {
        "archive" => {
            policy.style = FolderStyle::Refined;
            policy.rename_images_with_date = true;
        }
        "media" => {
            policy.style = FolderStyle::Refined;
            policy.use_subfolders = true;
            policy.rename_media = true;
        }
        _ => {}
    }
    scan.protect_projects = true;
    Some((scan, policy))
}

fn resolve_policy(
    client: &EngineClient,
    preset: &str,
) -> Result<(ScanOptions, ProposalPolicy), String> {
    if let Some(pair) = policy_for_preset(preset) {
        return Ok(pair);
    }
    let settings = client.get_settings().map_err(|error| error.to_string())?;
    Ok((settings.scan, settings.policy))
}

#[tauri::command]
fn scan_root(
    app: AppHandle,
    state: State<EngineState>,
    args: ScanArgs,
) -> Result<WorkspaceSnapshot, String> {
    ensure_client(&state)?;
    with_client(&state, |client| {
        let (options, _) = resolve_policy(client, &args.preset)?;
        let envelopes = client
            .request_with_events(
                aifs_protocol::Command::Scan {
                    root: args.root.clone().into(),
                    options,
                    session: None,
                },
                |envelope| {
                    forward_engine_event(&app, &envelope.event);
                },
            )
            .map_err(|error| error.to_string())?;
        for envelope in envelopes {
            match envelope.event {
                Event::ScanCompleted { snapshot } => return Ok(snapshot),
                Event::Cancelled => return Err("request cancelled".to_owned()),
                Event::Failed { message, .. } => return Err(message),
                _ => {}
            }
        }
        Err("scan ended without a snapshot".to_owned())
    })
}

#[tauri::command]
fn propose_session(
    state: State<EngineState>,
    session: SessionId,
    preset: String,
) -> Result<ProposalRevision, String> {
    with_client(&state, |client| {
        let (_, policy) = resolve_policy(client, &preset)?;
        client
            .propose(session, policy)
            .map_err(|error| error.to_string())
    })
}

#[tauri::command]
fn patch_revision(
    state: State<EngineState>,
    session: SessionId,
    base_revision: RevisionId,
    summary: String,
    patches: Vec<RevisionPatch>,
) -> Result<ProposalRevision, String> {
    with_client(&state, |client| {
        client
            .patch(
                session,
                base_revision,
                RevisionAuthor::User,
                summary,
                patches,
            )
            .map_err(|error| error.to_string())
    })
}

#[derive(Serialize)]
struct PlanResult {
    plan: Option<OperationPlan>,
    issues: Vec<PlanIssue>,
}

#[tauri::command]
fn plan_revision(
    state: State<EngineState>,
    session: SessionId,
    revision: RevisionId,
) -> Result<PlanResult, String> {
    with_client(&state, |client| {
        let envelopes = client
            .request_with_events(aifs_protocol::Command::Plan { session, revision }, |_| {})
            .map_err(|error| error.to_string())?;
        for envelope in envelopes {
            match envelope.event {
                Event::Planned { plan, issues } => {
                    return Ok(PlanResult {
                        plan: Some(plan),
                        issues,
                    });
                }
                Event::Failed {
                    message, issues, ..
                } => {
                    return Ok(PlanResult {
                        plan: None,
                        issues: if issues.is_empty() {
                            vec![PlanIssue::error("plan_rejected", message, Vec::new())]
                        } else {
                            issues
                        },
                    });
                }
                _ => {}
            }
        }
        Err("plan ended without a result".to_owned())
    })
}

#[tauri::command]
fn apply_plan(
    app: AppHandle,
    state: State<EngineState>,
    session: SessionId,
    plan: PlanId,
    dry_run: bool,
) -> Result<ApplyJournal, String> {
    with_client(&state, |client| {
        let envelopes = client
            .request_with_events(
                aifs_protocol::Command::Apply {
                    session,
                    plan,
                    dry_run,
                },
                |envelope| {
                    forward_engine_event(&app, &envelope.event);
                },
            )
            .map_err(|error| error.to_string())?;
        for envelope in envelopes {
            match envelope.event {
                Event::Journal { journal } => return Ok(journal),
                Event::Cancelled => return Err("request cancelled".to_owned()),
                Event::Failed { message, .. } => return Err(message),
                _ => {}
            }
        }
        Err("apply ended without a journal".to_owned())
    })
}

#[tauri::command]
fn undo_journal(
    state: State<EngineState>,
    session: SessionId,
    journal: JournalId,
) -> Result<ApplyJournal, String> {
    with_client(&state, |client| {
        client
            .undo(session, journal)
            .map_err(|error| error.to_string())
    })
}

#[derive(Serialize)]
struct ChatResult {
    message: String,
    revision: Option<ProposalRevision>,
}

#[tauri::command]
fn chat_revision(
    state: State<EngineState>,
    session: SessionId,
    revision: RevisionId,
    utterance: String,
) -> Result<ChatResult, String> {
    with_client(&state, |client| {
        let reply = client
            .chat(session, revision, utterance)
            .map_err(|error| error.to_string())?;
        Ok(ChatResult {
            message: reply.message,
            revision: reply.revision,
        })
    })
}

#[tauri::command]
fn cancel_in_flight(state: State<EngineState>) -> Result<(), String> {
    let client = clone_client(&state)?;
    client.cancel_in_flight().map_err(|error| error.to_string())
}

#[tauri::command]
fn get_settings(state: State<EngineState>) -> Result<AppSettings, String> {
    ensure_client(&state)?;
    with_client(&state, |client| {
        client.get_settings().map_err(|error| error.to_string())
    })
}

#[tauri::command]
fn put_settings(state: State<EngineState>, settings: AppSettings) -> Result<AppSettings, String> {
    ensure_client(&state)?;
    with_client(&state, |client| {
        client
            .put_settings(settings)
            .map_err(|error| error.to_string())
    })
}

#[tauri::command]
fn get_models(state: State<EngineState>) -> Result<ModelInventory, String> {
    ensure_client(&state)?;
    with_client(&state, |client| {
        client.get_models().map_err(|error| error.to_string())
    })
}

#[tauri::command]
fn put_models(
    state: State<EngineState>,
    inventory: ModelInventory,
) -> Result<ModelInventory, String> {
    ensure_client(&state)?;
    with_client(&state, |client| {
        client
            .put_models(inventory)
            .map_err(|error| error.to_string())
    })
}

#[derive(Debug, Deserialize)]
struct ProbeArgs {
    #[serde(flatten)]
    backend: ModelBackend,
    #[serde(default)]
    api_key: Option<String>,
}

#[tauri::command]
fn probe_endpoint(state: State<EngineState>, args: ProbeArgs) -> Result<(bool, String), String> {
    ensure_client(&state)?;
    with_client(&state, |client| {
        client
            .probe_endpoint(args.backend, args.api_key)
            .map_err(|error| error.to_string())
    })
}

#[tauri::command]
fn download_model(
    app: AppHandle,
    state: State<EngineState>,
    catalog_id: String,
) -> Result<ModelInventory, String> {
    ensure_client(&state)?;
    with_client(&state, |client| {
        let envelopes = client
            .request_with_events(
                aifs_protocol::Command::DownloadModel { catalog_id },
                |envelope| {
                    forward_engine_event(&app, &envelope.event);
                },
            )
            .map_err(|error| error.to_string())?;
        for envelope in envelopes {
            match envelope.event {
                Event::Models { inventory } => return Ok(inventory),
                Event::Failed { message, .. } => return Err(message),
                _ => {}
            }
        }
        Err("download ended without models".to_owned())
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(EngineState {
            client: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            connect_engine,
            pick_folder,
            scan_root,
            propose_session,
            patch_revision,
            plan_revision,
            apply_plan,
            undo_journal,
            chat_revision,
            cancel_in_flight,
            get_settings,
            put_settings,
            get_models,
            put_models,
            download_model,
            probe_endpoint
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
