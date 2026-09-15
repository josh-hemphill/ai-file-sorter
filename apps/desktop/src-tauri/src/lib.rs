//! Tauri shell: launches `aifs-engine` and forwards typed commands. No scan or
//! mutation happens in this process beyond spawning the engine.
//!
//! Engine I/O is blocking JSONL. Commands that wait on it run on Tokio's
//! blocking pool so the WebView event loop stays free. Native folder dialogs
//! and cancel stay on the UI thread (dialogs require it; cancel must not queue
//! behind a download).

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

/// Shared engine child. `Clone` so async commands can move a handle into `spawn_blocking`.
#[derive(Clone)]
struct EngineState {
    client: Arc<Mutex<Option<Arc<EngineClient>>>>,
    settings: Arc<Mutex<Option<AppSettings>>>,
    models: Arc<Mutex<Option<ModelInventory>>>,
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
    {
        let guard = state
            .client
            .lock()
            .map_err(|_| "engine lock poisoned".to_owned())?;
        if guard.is_some() {
            return Ok(());
        }
    }
    let binary = discover_engine_binary().map_err(|error| {
        if cfg!(debug_assertions) {
            error.to_string()
        } else {
            format!("aifs-engine sidecar is missing from this bundle ({error})")
        }
    })?;
    let client =
        EngineClient::connect(binary, "aifs-desktop").map_err(|error| error.to_string())?;
    let mut guard = state
        .client
        .lock()
        .map_err(|_| "engine lock poisoned".to_owned())?;
    if guard.is_some() {
        return Ok(());
    }
    *guard = Some(Arc::new(client));
    Ok(())
}

fn with_ready_client<T>(
    state: &EngineState,
    fun: impl FnOnce(&EngineClient) -> Result<T, String>,
) -> Result<T, String> {
    ensure_client(state)?;
    with_client(state, fun)
}

/// Returns `cache` when the engine is busy with scan/download; otherwise fetches.
fn cached_or_fetch<T: Clone>(
    busy: bool,
    cache: &Mutex<Option<T>>,
    fetch: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if busy
        && let Ok(guard) = cache.lock()
        && let Some(value) = guard.as_ref()
    {
        return Ok(value.clone());
    }
    let value = fetch()?;
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(value.clone());
    }
    Ok(value)
}

fn remember<T: Clone>(cache: &Mutex<Option<T>>, value: &T) {
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(value.clone());
    }
}

/// Runs blocking engine stdio off the WebView thread.
async fn run_blocking<T, F>(fun: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(fun)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn connect_engine(state: State<'_, EngineState>) -> Result<(), String> {
    let state = state.inner().clone();
    run_blocking(move || ensure_client(&state)).await
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
    #[serde(default)]
    session: Option<SessionId>,
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
async fn scan_root(
    app: AppHandle,
    state: State<'_, EngineState>,
    args: ScanArgs,
) -> Result<WorkspaceSnapshot, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
            let (options, _) = resolve_policy(client, &args.preset)?;
            let envelopes = client
                .request_with_events(
                    aifs_protocol::Command::Scan {
                        root: args.root.clone().into(),
                        options,
                        session: args.session,
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
    })
    .await
}

#[tauri::command]
async fn propose_session(
    state: State<'_, EngineState>,
    session: SessionId,
    preset: String,
) -> Result<ProposalRevision, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
            let (_, policy) = resolve_policy(client, &preset)?;
            client
                .propose(session, policy)
                .map_err(|error| error.to_string())
        })
    })
    .await
}

#[tauri::command]
async fn patch_revision(
    state: State<'_, EngineState>,
    session: SessionId,
    base_revision: RevisionId,
    summary: String,
    patches: Vec<RevisionPatch>,
) -> Result<ProposalRevision, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
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
    })
    .await
}

#[derive(Serialize)]
struct PlanResult {
    plan: Option<OperationPlan>,
    issues: Vec<PlanIssue>,
}

#[tauri::command]
async fn plan_revision(
    state: State<'_, EngineState>,
    session: SessionId,
    revision: RevisionId,
) -> Result<PlanResult, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
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
    })
    .await
}

#[tauri::command]
async fn apply_plan(
    app: AppHandle,
    state: State<'_, EngineState>,
    session: SessionId,
    plan: PlanId,
    dry_run: bool,
) -> Result<ApplyJournal, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
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
    })
    .await
}

#[tauri::command]
async fn undo_journal(
    state: State<'_, EngineState>,
    session: SessionId,
    journal: JournalId,
) -> Result<ApplyJournal, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
            client
                .undo(session, journal)
                .map_err(|error| error.to_string())
        })
    })
    .await
}

#[derive(Serialize)]
struct ChatResult {
    message: String,
    revision: Option<ProposalRevision>,
}

#[tauri::command]
async fn chat_revision(
    state: State<'_, EngineState>,
    session: SessionId,
    revision: RevisionId,
    utterance: String,
) -> Result<ChatResult, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
            let reply = client
                .chat(session, revision, utterance)
                .map_err(|error| error.to_string())?;
            Ok(ChatResult {
                message: reply.message,
                revision: reply.revision,
            })
        })
    })
    .await
}

/// Stays on the UI thread so cancel is not queued behind a blocking scan/download.
#[tauri::command]
fn cancel_in_flight(state: State<EngineState>) -> Result<(), String> {
    let client = clone_client(&state)?;
    client.cancel_in_flight().map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_settings(state: State<'_, EngineState>) -> Result<AppSettings, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
            cached_or_fetch(client.is_busy(), &state.settings, || {
                client.get_settings().map_err(|error| error.to_string())
            })
        })
    })
    .await
}

#[tauri::command]
async fn put_settings(
    state: State<'_, EngineState>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
            let settings = client
                .put_settings(settings)
                .map_err(|error| error.to_string())?;
            remember(&state.settings, &settings);
            Ok(settings)
        })
    })
    .await
}

#[tauri::command]
async fn get_models(state: State<'_, EngineState>) -> Result<ModelInventory, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
            cached_or_fetch(client.is_busy(), &state.models, || {
                client.get_models().map_err(|error| error.to_string())
            })
        })
    })
    .await
}

#[tauri::command]
async fn put_models(
    state: State<'_, EngineState>,
    inventory: ModelInventory,
) -> Result<ModelInventory, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
            let inventory = client
                .put_models(inventory)
                .map_err(|error| error.to_string())?;
            remember(&state.models, &inventory);
            Ok(inventory)
        })
    })
    .await
}

#[derive(Debug, Deserialize)]
struct ProbeArgs {
    #[serde(flatten)]
    backend: ModelBackend,
    #[serde(default)]
    api_key: Option<String>,
}

#[tauri::command]
async fn probe_endpoint(
    state: State<'_, EngineState>,
    args: ProbeArgs,
) -> Result<(bool, String), String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
            client
                .probe_endpoint(args.backend, args.api_key)
                .map_err(|error| error.to_string())
        })
    })
    .await
}

#[tauri::command]
async fn download_model(
    app: AppHandle,
    state: State<'_, EngineState>,
    catalog_id: String,
) -> Result<ModelInventory, String> {
    let state = state.inner().clone();
    run_blocking(move || {
        with_ready_client(&state, |client| {
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
                    Event::Models { inventory } => {
                        remember(&state.models, &inventory);
                        return Ok(inventory);
                    }
                    Event::Cancelled => return Err("request cancelled".to_owned()),
                    Event::Failed { message, .. } => return Err(message),
                    _ => {}
                }
            }
            Err("download ended without models".to_owned())
        })
    })
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(EngineState {
            client: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(None)),
            models: Arc::new(Mutex::new(None)),
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

#[cfg(test)]
mod tests {
    #[test]
    fn bundle_embeds_engine_workers_and_sets_csp() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !conf["app"]["security"]["csp"].is_null(),
            "csp must not be null"
        );
        let csp = conf["app"]["security"]["csp"]
            .as_object()
            .unwrap_or_else(|| panic!("csp must be an object allow-list"));
        for key in [
            "default-src",
            "connect-src",
            "img-src",
            "style-src",
            "script-src",
        ] {
            let value = csp
                .get(key)
                .and_then(|item| item.as_str())
                .unwrap_or_else(|| panic!("csp.{key}"));
            match key {
                "default-src" | "script-src" => {
                    assert!(value.contains("'self'"), "{key}={value}");
                }
                "connect-src" => {
                    assert!(value.contains("ipc:"), "{key}={value}");
                }
                "img-src" => {
                    assert!(
                        value.contains("'self'") && value.contains("asset:"),
                        "{key}={value}"
                    );
                }
                "style-src" => {
                    assert!(
                        value.contains("'self'") && value.contains("'unsafe-inline'"),
                        "{key}={value}"
                    );
                }
                _ => {}
            }
        }
        let bins = conf["bundle"]["externalBin"]
            .as_array()
            .unwrap_or_else(|| panic!("externalBin"));
        let expected = [
            "binaries/aifs-engine",
            "binaries/aifs-worker-media",
            "binaries/aifs-worker-document",
            "binaries/aifs-worker-vision",
        ];
        assert_eq!(bins.len(), expected.len(), "{bins:?}");
        for name in expected {
            assert!(
                bins.iter().any(|value| value.as_str() == Some(name)),
                "missing {name} in {bins:?}"
            );
        }
        assert!(
            bins.iter()
                .all(|value| value.as_str() != Some("binaries/aifs-worker-llm")),
            "LLM worker must not be a flat externalBin sidecar: {bins:?}"
        );
        let resources = conf["bundle"]["resources"]
            .as_object()
            .unwrap_or_else(|| panic!("resources"));
        assert_eq!(
            resources
                .get("resources/llm-runtime")
                .and_then(|value| value.as_str()),
            Some("llm-runtime"),
            "Tauri glob maps flatten to dest.join(file_name); walk the directory so <accel>/ is kept: {resources:?}"
        );
        assert!(
            resources
                .keys()
                .all(|key| !key.contains("llm-runtime") || !key.contains('*')),
            "llm-runtime glob resources flatten sibling accelerators: {resources:?}"
        );
    }

    #[test]
    fn engine_bins_builds_the_engine_binary_package() {
        let alias = include_str!("../../../../.cargo/config.toml")
            .lines()
            .find(|line| line.starts_with("engine-bins"))
            .unwrap_or_else(|| panic!("engine-bins alias"));
        assert!(
            alias.contains("-p aifs-engine-bin"),
            "engine-bins must build bins/aifs-engine, not the library crate: {alias}"
        );
        assert!(
            !alias.split_whitespace().any(|token| token == "aifs-engine"),
            "engine-bins must not pass -p aifs-engine: {alias}"
        );
        let packages = include_str!("../../../../package.json");
        assert!(
            packages.contains("\"build\": \"cargo engine-bins\""),
            "pnpm build must compile the engine binary crates: {packages}"
        );
        let engine_llm = include_str!("../../../../.cargo/config.toml")
            .lines()
            .find(|line| line.starts_with("engine-llm"))
            .unwrap_or_else(|| panic!("engine-llm alias"));
        assert!(
            engine_llm.contains("-p aifs-worker-llm") && engine_llm.contains("--features llama"),
            "engine-llm must build the LLM worker with llama.cpp: {engine_llm}"
        );
        let makefile = include_str!("../../../../Makefile");
        assert!(
            makefile.contains("pnpm --filter desktop tauri dev"),
            "make desktop starts Tauri; llama is skipped when llm-runtime is already staged: {makefile}"
        );
        assert!(
            makefile.contains("node scripts/with-cmake-generator.mjs cargo engine-llm"),
            "make llama must wrap cargo engine-llm: {makefile}"
        );
        assert!(
            makefile.contains("with-llm-features.mjs cuda")
                && makefile.contains("desktop-cuda:")
                && makefile.contains("llama-vulkan:"),
            "make desktop-cuda / llama-vulkan must set AIFS_LLM_FEATURES: {makefile}"
        );
        let pkg = include_str!("../../../../package.json");
        assert!(
            pkg.contains("desktop:open")
                && pkg.contains("\"desktop\": \"pnpm build && pnpm --filter desktop tauri dev\"")
                && pkg.contains("pnpm llama")
                && pkg.contains("tauri dev"),
            "pnpm desktop reuses a staged llm-runtime; pnpm llama still rebuilds: {pkg}"
        );
        assert!(
            pkg.contains("desktop:cuda")
                && pkg.contains("desktop:vulkan")
                && pkg.contains("with-llm-features.mjs"),
            "pnpm desktop:cuda / desktop:vulkan must set AIFS_LLM_FEATURES: {pkg}"
        );
        let cmake_wrap = include_str!("../../../../scripts/with-cmake-generator.mjs");
        assert!(
            cmake_wrap.contains("appendEngineLlmFeatures")
                && cmake_wrap.contains("formatStagedPayloadLog"),
            "Tauri cargo engine-llm must inherit AIFS_LLM_FEATURES and log staged libs: {cmake_wrap}"
        );
        let stage = include_str!("../../../../scripts/stage-llm-payload.mjs");
        assert!(
            stage.contains("collectRuntimeLibsNested") && stage.contains("expectedAccel"),
            "CUDA staging must harvest nested llama-cpp out dirs and fail closed: {stage}"
        );
        let tauri = include_str!("../tauri.conf.json");
        let before_dev = tauri
            .lines()
            .find(|row| row.contains("beforeDevCommand"))
            .unwrap_or_else(|| panic!("beforeDevCommand"));
        assert!(
            before_dev.contains("cargo engine-bins")
                && before_dev.contains("ensure-llm-worker.mjs"),
            "beforeDevCommand skips cargo engine-llm when a payload is staged: {before_dev}"
        );
        let before_build = tauri
            .lines()
            .find(|row| row.contains("beforeBuildCommand"))
            .unwrap_or_else(|| panic!("beforeBuildCommand"));
        let bins = before_build.find("cargo engine-bins").unwrap_or_else(|| {
            panic!("beforeBuildCommand must run cargo engine-bins: {before_build}")
        });
        let wrap = before_build.find("with-cmake-generator.mjs").unwrap_or_else(|| {
            panic!("beforeBuildCommand must wrap cargo engine-llm for Windows CMake: {before_build}")
        });
        let llm = before_build.find("cargo engine-llm").unwrap_or_else(|| {
            panic!("beforeBuildCommand must run cargo engine-llm: {before_build}")
        });
        assert!(
            bins < wrap && wrap < llm,
            "beforeBuildCommand must overwrite the stub worker with llama.cpp: {before_build}"
        );
        let build = include_str!("../build.rs");
        assert!(
            !build.contains("src_dir.display()"),
            "watching the whole target profile directory retriggers tauri dev: {build}"
        );
        assert!(
            build.contains("copy_if_changed")
                && build.contains("write_if_changed")
                && build.contains("stage_llm_payload")
                && !build.contains("copy_worker_runtime_libs"),
            "sidecar copies must skip identical destinations and stage llm-runtime/<accel>/ without a flat LLM sidecar: {build}"
        );
        let ignore = include_str!("../.taurignore");
        assert!(
            ignore.contains("binaries/")
                && ignore.contains("gen/")
                && ignore.contains("resources/llm-runtime/"),
            "tauri dev must ignore sidecar copies, runtime libs, and generated schemas: {ignore}"
        );
    }

    #[test]
    fn engine_io_commands_are_async_and_folder_dialog_stays_sync() {
        let src = include_str!("lib.rs");
        let impl_src = src
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_else(|| panic!("production source"));
        assert!(
            impl_src.contains("spawn_blocking"),
            "engine I/O must use spawn_blocking so the WebView thread stays free"
        );
        for name in [
            "connect_engine",
            "scan_root",
            "propose_session",
            "patch_revision",
            "plan_revision",
            "apply_plan",
            "undo_journal",
            "chat_revision",
            "get_settings",
            "put_settings",
            "get_models",
            "put_models",
            "probe_endpoint",
            "download_model",
        ] {
            let async_fn = format!("async fn {name}");
            assert!(
                impl_src.contains(&async_fn),
                "{name} must be async so Tauri does not run it on the UI thread"
            );
        }
        assert!(
            impl_src.contains("fn pick_folder(") && !impl_src.contains("async fn pick_folder"),
            "pick_folder must stay sync; native dialogs need the UI thread"
        );
        assert!(
            impl_src.contains("fn cancel_in_flight(")
                && !impl_src.contains("async fn cancel_in_flight"),
            "cancel_in_flight must stay sync so it is not queued behind a blocking worker"
        );
    }

    #[test]
    fn cached_or_fetch_returns_cache_when_busy() {
        use super::cached_or_fetch;
        use std::sync::Mutex;
        let cache = Mutex::new(Some("cached".to_owned()));
        let value = cached_or_fetch(true, &cache, || panic!("must not fetch while busy"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(value, "cached");
        let fresh = cached_or_fetch(false, &cache, || Ok("fresh".to_owned()))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(fresh, "fresh");
        assert_eq!(
            cache
                .lock()
                .unwrap_or_else(|error| panic!("{error}"))
                .as_deref(),
            Some("fresh")
        );
    }
}

#[cfg(test)]
#[path = "../sidecar_copy.rs"]
mod sidecar_copy;
