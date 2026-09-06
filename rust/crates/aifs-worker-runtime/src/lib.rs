//! Shared stdin/stdout loop for worker binaries.

use aifs_domain::{Evidence, ObservedEntry};
use aifs_protocol::worker::{
    WORKER_PROTOCOL_VERSION, WorkerCommand, WorkerEnvelope, WorkerEvent, WorkerKind, WorkerRequest,
};
use aifs_protocol::{ErrorCode, ModelBackend, decode_line, encode_line};
use std::io::{self, BufRead, Write};
use std::path::Path;

/// Handles worker commands after `hello`. Extract-only workers implement only [`Self::extract`].
pub trait WorkerHandler {
    /// Deterministic extract. Default: unsupported.
    fn extract(&mut self, root: &Path, entry: &ObservedEntry) -> Result<Option<Evidence>, String> {
        let _ = (root, entry);
        Err("extract is not supported".to_owned())
    }

    /// Bind a backend. Default: unsupported.
    fn load(
        &mut self,
        backend: ModelBackend,
        gpu_preference: &str,
        n_gpu_layers: Option<u32>,
        api_key: Option<String>,
        storage_dir: &str,
    ) -> Result<LoadedModel, String> {
        let _ = (backend, gpu_preference, n_gpu_layers, api_key, storage_dir);
        Err("load is not supported".to_owned())
    }

    /// Drop a loaded backend.
    fn unload(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// Categorize a file. Default: unsupported.
    fn categorize(
        &mut self,
        root: &Path,
        entry: &ObservedEntry,
        evidence: &[Evidence],
    ) -> Result<Option<Evidence>, String> {
        let _ = (root, entry, evidence);
        Err("categorize is not supported".to_owned())
    }

    /// Describe an image. Default: unsupported.
    fn describe(
        &mut self,
        root: &Path,
        entry: &ObservedEntry,
        evidence: &[Evidence],
    ) -> Result<Option<Evidence>, String> {
        let _ = (root, entry, evidence);
        Err("describe is not supported".to_owned())
    }

    /// Assistant turn. Default: unsupported.
    fn chat(&mut self, utterance: &str, context: &str) -> Result<String, String> {
        let _ = (utterance, context);
        Err("chat is not supported".to_owned())
    }
}

/// Result of a successful [`WorkerHandler::load`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedModel {
    /// Device actually used (`cpu`, `cuda`, `vulkan`, `metal`, `stub`).
    pub device: String,
    /// Model id or filename.
    pub model: String,
    /// Layers offloaded.
    pub n_gpu_layers: u32,
    /// Why a requested accelerator was not used.
    pub fallback: Option<String>,
}

struct ExtractFn<F>(F);

impl<F> WorkerHandler for ExtractFn<F>
where
    F: FnMut(&Path, &ObservedEntry) -> Result<Option<Evidence>, String>,
{
    fn extract(&mut self, root: &Path, entry: &ObservedEntry) -> Result<Option<Evidence>, String> {
        (self.0)(root, entry)
    }
}

/// Runs a worker until `shutdown` or stdin closes, using an extract closure.
pub fn run_stdio(
    kind: WorkerKind,
    capabilities: &[&str],
    extract: impl FnMut(&Path, &ObservedEntry) -> Result<Option<Evidence>, String>,
) -> io::Result<()> {
    run_with_handler(kind, capabilities, ExtractFn(extract))
}

/// Runs a worker until `shutdown` or stdin closes.
pub fn run_with_handler(
    kind: WorkerKind,
    capabilities: &[&str],
    mut handler: impl WorkerHandler,
) -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut hello_ok = false;
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: WorkerRequest = match decode_line(&line) {
            Ok(request) => request,
            Err(error) => {
                write_envelope(
                    &mut stdout,
                    &WorkerEnvelope::reply(
                        &"invalid".into(),
                        WorkerEvent::Failed {
                            code: ErrorCode::InvalidRequest,
                            message: error.to_string(),
                        },
                    ),
                )?;
                continue;
            }
        };
        match request.command {
            WorkerCommand::Shutdown => {
                write_envelope(
                    &mut stdout,
                    &WorkerEnvelope::reply(&request.id, WorkerEvent::Shutdown),
                )?;
                return Ok(());
            }
            other => {
                let envelope = dispatch(
                    kind,
                    capabilities,
                    &mut hello_ok,
                    &request.id,
                    other,
                    &mut handler,
                );
                write_envelope(&mut stdout, &envelope)?;
            }
        }
    }
    Ok(())
}

fn dispatch(
    kind: WorkerKind,
    capabilities: &[&str],
    hello_ok: &mut bool,
    id: &aifs_protocol::RequestId,
    command: WorkerCommand,
    handler: &mut impl WorkerHandler,
) -> WorkerEnvelope {
    if let WorkerCommand::Hello {
        worker,
        protocol_version,
    } = command
    {
        return handle_hello(kind, capabilities, hello_ok, id, worker, protocol_version);
    }
    if !*hello_ok {
        return WorkerEnvelope::reply(
            id,
            WorkerEvent::Failed {
                code: ErrorCode::InvalidRequest,
                message: "send hello first".to_owned(),
            },
        );
    }
    match command {
        WorkerCommand::Hello { .. } => unreachable!("hello handled above"),
        WorkerCommand::Extract { root, entry } => {
            result_extracted(id, handler.extract(&root, &entry))
        }
        WorkerCommand::Load {
            backend,
            gpu_preference,
            n_gpu_layers,
            api_key,
            storage_dir,
        } => match handler.load(
            backend,
            &gpu_preference,
            n_gpu_layers,
            api_key
                .filter(|secret| !secret.is_blank())
                .map(aifs_protocol::worker::RedactedString::into_inner),
            &storage_dir,
        ) {
            Ok(loaded) => WorkerEnvelope::reply(
                id,
                WorkerEvent::Loaded {
                    device: loaded.device,
                    model: loaded.model,
                    n_gpu_layers: loaded.n_gpu_layers,
                    fallback: loaded.fallback,
                },
            ),
            Err(message) => failed(id, message),
        },
        WorkerCommand::Unload => match handler.unload() {
            Ok(()) => WorkerEnvelope::reply(id, WorkerEvent::Unloaded),
            Err(message) => failed(id, message),
        },
        WorkerCommand::Categorize {
            root,
            entry,
            evidence,
        } => result_inferred(id, handler.categorize(&root, &entry, &evidence)),
        WorkerCommand::Describe {
            root,
            entry,
            evidence,
        } => result_inferred(id, handler.describe(&root, &entry, &evidence)),
        WorkerCommand::Chat { utterance, context } => match handler.chat(&utterance, &context) {
            Ok(message) => WorkerEnvelope::reply(id, WorkerEvent::ChatCompleted { message }),
            Err(message) => failed(id, message),
        },
        WorkerCommand::Shutdown => unreachable!("shutdown handled in the loop"),
    }
}

fn handle_hello(
    kind: WorkerKind,
    capabilities: &[&str],
    hello_ok: &mut bool,
    id: &aifs_protocol::RequestId,
    worker: WorkerKind,
    protocol_version: u32,
) -> WorkerEnvelope {
    if worker != kind {
        return WorkerEnvelope::reply(
            id,
            WorkerEvent::Failed {
                code: ErrorCode::InvalidRequest,
                message: format!("this process is {kind:?}, engine asked for {worker:?}"),
            },
        );
    }
    if protocol_version != WORKER_PROTOCOL_VERSION {
        return WorkerEnvelope::reply(
            id,
            WorkerEvent::Failed {
                code: ErrorCode::IncompatibleProtocol,
                message: format!(
                    "worker speaks {WORKER_PROTOCOL_VERSION}, engine spoke {protocol_version}"
                ),
            },
        );
    }
    *hello_ok = true;
    WorkerEnvelope::reply(
        id,
        WorkerEvent::Ready {
            worker: kind,
            protocol_version: WORKER_PROTOCOL_VERSION,
            capabilities: capabilities.iter().map(|cap| (*cap).to_owned()).collect(),
        },
    )
}

fn result_extracted(
    id: &aifs_protocol::RequestId,
    result: Result<Option<Evidence>, String>,
) -> WorkerEnvelope {
    match result {
        Ok(evidence) => WorkerEnvelope::reply(id, WorkerEvent::Extracted { evidence }),
        Err(message) => failed(id, message),
    }
}

fn result_inferred(
    id: &aifs_protocol::RequestId,
    result: Result<Option<Evidence>, String>,
) -> WorkerEnvelope {
    match result {
        Ok(evidence) => WorkerEnvelope::reply(id, WorkerEvent::Inferred { evidence }),
        Err(message) => failed(id, message),
    }
}

fn failed(id: &aifs_protocol::RequestId, message: String) -> WorkerEnvelope {
    WorkerEnvelope::reply(
        id,
        WorkerEvent::Failed {
            code: ErrorCode::Io,
            message,
        },
    )
}

fn write_envelope(stdout: &mut impl Write, envelope: &WorkerEnvelope) -> io::Result<()> {
    let encoded = encode_line(envelope)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    writeln!(stdout, "{encoded}")?;
    stdout.flush()
}
