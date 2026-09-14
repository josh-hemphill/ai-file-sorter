//! Discover complete `llm-runtime/<accel>/` payloads and pick one to spawn.

use crate::{WorkerClientError, discover_worker_binary};
use aifs_protocol::worker::WorkerKind;
use aifs_protocol::{
    LlmAccel, LlmPayload, explain_llm_payload_selection_failure, host_accel_available,
    infer_accel_from_libs, inspect_payload, is_staged_payload_dir, is_usable_process_binary,
    list_payloads_under, payload_dir_has_lib_prefix, select_llm_payload,
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

thread_local! {
    static LIST_ROOTS_OVERRIDE: RefCell<Option<Vec<PathBuf>>> = const { RefCell::new(None) };
}

/// Overrides `gpu_preference` when selecting which payload directory to spawn.
pub const LLM_BACKEND_ENV: &str = "AIFS_LLM_BACKEND";

/// Preference used for spawn: `AIFS_LLM_BACKEND` when set, else `gpu_preference`.
pub fn spawn_llm_preference(gpu_preference: &str) -> String {
    spawn_llm_preference_from(
        gpu_preference,
        std::env::var(LLM_BACKEND_ENV).ok().as_deref(),
    )
}

fn spawn_llm_preference_from(gpu_preference: &str, backend_env: Option<&str>) -> String {
    backend_env
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(gpu_preference)
        .to_owned()
}

/// Complete payloads under Cargo/Tauri search roots (no worker hello).
pub fn list_llm_payloads() -> Vec<LlmPayload> {
    list_llm_payloads_from(&current_list_roots())
}

fn current_list_roots() -> Vec<PathBuf> {
    LIST_ROOTS_OVERRIDE.with(|slot| match slot.borrow().as_ref() {
        Some(roots) => roots.clone(),
        None => runtime_search_roots(),
    })
}

/// Limits [`list_llm_payloads`] on this thread to `roots` until the guard drops.
///
/// Engine tests use this so a fake payload is not planted under `CARGO_MANIFEST_DIR`,
/// which parallel `connect_llm` tests also search.
pub fn override_llm_list_roots(roots: Vec<PathBuf>) -> LlmListRootsGuard {
    LlmListRootsGuard {
        previous: LIST_ROOTS_OVERRIDE.with(|slot| slot.replace(Some(roots))),
    }
}

/// Restores the previous listing-root override when dropped.
pub struct LlmListRootsGuard {
    previous: Option<Vec<PathBuf>>,
}

impl Drop for LlmListRootsGuard {
    fn drop(&mut self) {
        LIST_ROOTS_OVERRIDE.with(|slot| {
            *slot.borrow_mut() = self.previous.take();
        });
    }
}

/// Lists complete payloads under each runtime root (`root/llm-runtime/<accel>/`).
pub fn list_llm_payloads_from(roots: &[PathBuf]) -> Vec<LlmPayload> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        for payload in list_payloads_under(root) {
            if seen.insert(payload.dir.clone()) {
                out.push(payload);
            }
        }
    }
    out
}

/// Resolves the LLM worker payload to spawn.
///
/// `AIFS_WORKER_LLM` still wins (library search is that file's directory).
/// Otherwise autoselects a complete payload. An incomplete llama/CUDA cargo
/// sidecar is not spawned; a stub worker with no native libs still is.
pub fn discover_llm_payload(gpu_preference: &str) -> Result<LlmPayload, WorkerClientError> {
    if let Ok(explicit) = std::env::var(WorkerKind::Llm.env_var()) {
        return payload_from_explicit_binary(Path::new(&explicit));
    }
    let preference = spawn_llm_preference(gpu_preference);
    let roots = current_list_roots();
    let payloads = list_llm_payloads_from(&roots);
    if let Some(selected) = select_llm_payload(&payloads, &preference, host_accel_available) {
        return Ok(selected.clone());
    }
    let sidecar = discover_worker_binary(WorkerKind::Llm).ok();
    if let Some(binary) = &sidecar
        && let Some(payload) = complete_or_stub_sidecar(binary)
    {
        return Ok(payload);
    }
    Err(WorkerClientError::NoUsablePayload(
        explain_llm_payload_selection_failure(
            &roots,
            &preference,
            host_accel_available,
            sidecar.as_deref(),
        ),
    ))
}

fn payload_from_explicit_binary(path: &Path) -> Result<LlmPayload, WorkerClientError> {
    if !is_usable_process_binary(path) {
        return Err(WorkerClientError::NotFound(path.display().to_string()));
    }
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| path.to_path_buf());
    if let Some(accel) = infer_accel_from_libs(&dir)
        && let Some(mut payload) = inspect_payload(&dir, accel)
    {
        payload.binary = path.to_path_buf();
        return Ok(payload);
    }
    Ok(LlmPayload {
        accel: infer_accel_from_libs(&dir).unwrap_or(LlmAccel::Cpu),
        dir,
        binary: path.to_path_buf(),
    })
}

fn complete_sidecar_payload(binary: &Path) -> Option<LlmPayload> {
    if !is_usable_process_binary(binary) {
        return None;
    }
    let dir = binary.parent()?;
    let accel = infer_accel_from_libs(dir)?;
    let mut payload = inspect_payload(dir, accel)?;
    payload.binary = binary.to_path_buf();
    Some(payload)
}

/// Complete llama payload beside the cargo binary, or a stub worker with no native libs.
///
/// A CUDA-linked `target/debug/aifs-worker-llm.exe` that is missing llama/ggml
/// (or has `ggml-cuda` only in `deps/`) is not spawned.
fn complete_or_stub_sidecar(binary: &Path) -> Option<LlmPayload> {
    if let Some(payload) = complete_sidecar_payload(binary) {
        return Some(payload);
    }
    if !is_usable_process_binary(binary) {
        return None;
    }
    let dir = binary.parent()?;
    if is_staged_payload_dir(dir) || incomplete_native_sidecar(dir) {
        return None;
    }
    Some(LlmPayload {
        accel: LlmAccel::Cpu,
        dir: dir.to_path_buf(),
        binary: binary.to_path_buf(),
    })
}

fn incomplete_native_sidecar(dir: &Path) -> bool {
    if infer_accel_from_libs(dir).is_some() {
        return false;
    }
    nearby_lib_dirs(dir)
        .iter()
        .any(|candidate| sidecar_lib_prefixes_present(candidate))
}

fn nearby_lib_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![dir.to_path_buf(), dir.join("deps")];
    let Ok(entries) = std::fs::read_dir(dir.join("build")) else {
        return dirs;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if entry.path().is_dir()
            && name
                .to_str()
                .is_some_and(|stem| stem.to_ascii_lowercase().starts_with("llama-cpp"))
        {
            dirs.push(entry.path().join("out"));
        }
    }
    dirs
}

fn sidecar_lib_prefixes_present(dir: &Path) -> bool {
    ["llama", "ggml", "ggml-cuda", "ggml-vulkan"]
        .iter()
        .any(|prefix| payload_dir_has_lib_prefix(dir, prefix))
}

/// Directories that may contain `llm-runtime/<accel>/` (exe ancestors, `resources/`, Cargo target).
pub fn runtime_search_roots() -> Vec<PathBuf> {
    let mut starts = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        starts.push(dir.to_path_buf());
    }
    if let Some(manifest) = std::env::var_os("CARGO_MANIFEST_DIR") {
        starts.push(PathBuf::from(manifest));
    }
    runtime_search_roots_from(&starts)
}

fn runtime_search_roots_from(starts: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for start in starts {
        let mut dir = start.clone();
        for _ in 0..8 {
            push_resource_roots(&mut roots, &dir);
            for profile in ["debug", "release"] {
                push_unique(&mut roots, dir.join("target").join(profile));
            }
            if !dir.pop() {
                break;
            }
        }
    }
    roots
}

/// `dir`, `dir/resources`, `dir/Resources`, and macOS `Contents/Resources` beside `MacOS`.
fn push_resource_roots(roots: &mut Vec<PathBuf>, dir: &Path) {
    push_unique(roots, dir.to_path_buf());
    push_unique(roots, dir.join("resources"));
    push_unique(roots, dir.join("Resources"));
    if dir.file_name().is_some_and(|name| name == "MacOS")
        && let Some(parent) = dir.parent()
    {
        push_unique(roots, parent.join("Resources"));
    }
}

fn push_unique(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if !roots.contains(&path) {
        roots.push(path);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        complete_or_stub_sidecar, complete_sidecar_payload, list_llm_payloads,
        list_llm_payloads_from, override_llm_list_roots, payload_from_explicit_binary,
        runtime_search_roots_from, spawn_llm_preference_from,
    };
    use aifs_protocol::{LlmAccel, llm_payload_dir, select_llm_payload};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "aifs-llm-payload-discover-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
        dir
    }

    fn write_cpu_payload(dir: &Path) {
        fs::create_dir_all(dir).unwrap_or_else(|error| panic!("{error}"));
        fs::write(dir.join("aifs-worker-llm"), b"worker").unwrap_or_else(|error| panic!("{error}"));
        fs::write(dir.join("llama.dll"), b"llama").unwrap_or_else(|error| panic!("{error}"));
        fs::write(dir.join("ggml.dll"), b"ggml").unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn runtime_roots_include_resources_and_cargo_target() {
        let start = PathBuf::from("repo/apps/desktop/src-tauri/binaries");
        let roots = runtime_search_roots_from(&[start]);
        assert!(
            roots
                .iter()
                .any(|dir| dir == Path::new("repo/apps/desktop/src-tauri/resources")),
            "{roots:?}"
        );
        assert!(
            roots
                .iter()
                .any(|dir| dir == Path::new("repo/target/debug")),
            "{roots:?}"
        );
        assert!(
            roots
                .iter()
                .any(|dir| dir == Path::new("repo/apps/desktop/src-tauri/binaries")),
            "{roots:?}"
        );
    }

    #[test]
    fn list_llm_payloads_honors_thread_local_root_override() {
        let root = temp_dir();
        write_cpu_payload(&llm_payload_dir(&root, LlmAccel::Cpu));
        {
            let _empty = override_llm_list_roots(Vec::new());
            assert!(
                list_llm_payloads().is_empty(),
                "empty override must hide Cargo/Tauri search roots"
            );
        }
        let guard = override_llm_list_roots(vec![root.clone()]);
        let listed = list_llm_payloads();
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert!(
            listed[0].dir.starts_with(&root),
            "listed {:?} must be the staged root {root:?}",
            listed[0].dir
        );
        drop(guard);
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn runtime_roots_include_macos_contents_resources() {
        let start = PathBuf::from("App.app/Contents/MacOS");
        let roots = runtime_search_roots_from(&[start]);
        assert!(
            roots
                .iter()
                .any(|dir| dir == Path::new("App.app/Contents/Resources")),
            "packaged macOS resources live beside MacOS, not inside it: {roots:?}"
        );
        assert!(
            roots
                .iter()
                .any(|dir| dir == Path::new("App.app/Contents/MacOS")),
            "{roots:?}"
        );
    }

    #[test]
    fn list_from_roots_finds_nested_payloads_without_hello() {
        let root = temp_dir();
        write_cpu_payload(&llm_payload_dir(&root, LlmAccel::Cpu));
        write_cpu_payload(&llm_payload_dir(&root, LlmAccel::Cuda));
        fs::write(
            llm_payload_dir(&root, LlmAccel::Cuda).join("ggml-cuda.dll"),
            b"cuda",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let listed = list_llm_payloads_from(std::slice::from_ref(&root));
        let accels: Vec<_> = listed.iter().map(|payload| payload.accel).collect();
        assert!(accels.contains(&LlmAccel::Cpu), "{accels:?}");
        assert!(accels.contains(&LlmAccel::Cuda), "{accels:?}");
        let selected = select_llm_payload(&listed, "auto", |_| true)
            .unwrap_or_else(|| panic!("expected cuda"));
        assert_eq!(selected.accel, LlmAccel::Cuda);
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn explicit_worker_uses_its_directory_as_payload() {
        let root = temp_dir();
        let dir = llm_payload_dir(&root, LlmAccel::Cpu);
        write_cpu_payload(&dir);
        let binary = dir.join("aifs-worker-llm");
        let payload =
            payload_from_explicit_binary(&binary).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(payload.accel, LlmAccel::Cpu);
        assert_eq!(payload.dir, dir);
        assert_eq!(payload.binary, binary);
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn spawn_preference_uses_backend_env_unless_blank() {
        assert_eq!(spawn_llm_preference_from("cpu", Some("cuda")), "cuda");
        assert_eq!(
            spawn_llm_preference_from("cpu", Some("  vulkan  ")),
            "vulkan"
        );
        assert_eq!(spawn_llm_preference_from("cpu", Some("")), "cpu");
        assert_eq!(spawn_llm_preference_from("auto", None), "auto");
    }

    #[test]
    fn cargo_sidecar_is_spawnable_only_when_libs_sit_beside_the_worker() {
        let root = temp_dir();
        let binary = root.join("aifs-worker-llm");
        fs::write(&binary, b"worker").unwrap_or_else(|error| panic!("{error}"));
        assert!(
            complete_sidecar_payload(&binary).is_none(),
            "worker without llama/ggml is not a complete payload"
        );
        let stub = complete_or_stub_sidecar(&binary).unwrap_or_else(|| panic!("stub cargo worker"));
        assert_eq!(stub.accel, LlmAccel::Cpu);
        fs::create_dir_all(root.join("deps")).unwrap_or_else(|error| panic!("{error}"));
        fs::write(root.join("deps").join("ggml-cuda.dll"), b"cuda")
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            complete_or_stub_sidecar(&binary).is_none(),
            "CUDA plugin in deps means this is an incomplete llama sidecar"
        );
        write_cpu_payload(&root);
        let cpu = complete_sidecar_payload(&binary).unwrap_or_else(|| panic!("cpu sidecar"));
        assert_eq!(cpu.accel, LlmAccel::Cpu);
        fs::write(root.join("ggml-cuda.dll"), b"cuda").unwrap_or_else(|error| panic!("{error}"));
        let cuda = complete_sidecar_payload(&binary).unwrap_or_else(|| panic!("cuda sidecar"));
        assert_eq!(cuda.accel, LlmAccel::Cuda);
        fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }
}
