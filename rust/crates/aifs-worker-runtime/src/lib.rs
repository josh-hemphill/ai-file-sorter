//! Shared stdin/stdout loop for worker binaries.

use aifs_domain::{Evidence, ObservedEntry};
use aifs_protocol::worker::{
    WORKER_PROTOCOL_VERSION, WorkerCommand, WorkerEnvelope, WorkerEvent, WorkerKind, WorkerRequest,
};
use aifs_protocol::{ErrorCode, decode_line, encode_line};
use std::io::{self, BufRead, Write};
use std::path::Path;

/// Runs a worker until `shutdown` or stdin closes.
pub fn run_stdio(
    kind: WorkerKind,
    capabilities: &[&str],
    mut extract: impl FnMut(&Path, &ObservedEntry) -> Result<Option<Evidence>, String>,
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
        let envelope = match request.command {
            WorkerCommand::Hello {
                worker,
                protocol_version,
            } => {
                if worker != kind {
                    WorkerEnvelope::reply(
                        &request.id,
                        WorkerEvent::Failed {
                            code: ErrorCode::InvalidRequest,
                            message: format!(
                                "this process is {kind:?}, engine asked for {worker:?}"
                            ),
                        },
                    )
                } else if protocol_version != WORKER_PROTOCOL_VERSION {
                    WorkerEnvelope::reply(
                        &request.id,
                        WorkerEvent::Failed {
                            code: ErrorCode::IncompatibleProtocol,
                            message: format!(
                                "worker speaks {WORKER_PROTOCOL_VERSION}, engine spoke {protocol_version}"
                            ),
                        },
                    )
                } else {
                    hello_ok = true;
                    WorkerEnvelope::reply(
                        &request.id,
                        WorkerEvent::Ready {
                            worker: kind,
                            protocol_version: WORKER_PROTOCOL_VERSION,
                            capabilities: capabilities.iter().map(|c| (*c).to_owned()).collect(),
                        },
                    )
                }
            }
            WorkerCommand::Extract { root, entry } => {
                if !hello_ok {
                    WorkerEnvelope::reply(
                        &request.id,
                        WorkerEvent::Failed {
                            code: ErrorCode::InvalidRequest,
                            message: "send hello first".to_owned(),
                        },
                    )
                } else {
                    match extract(&root, &entry) {
                        Ok(evidence) => {
                            WorkerEnvelope::reply(&request.id, WorkerEvent::Extracted { evidence })
                        }
                        Err(message) => WorkerEnvelope::reply(
                            &request.id,
                            WorkerEvent::Failed {
                                code: ErrorCode::Io,
                                message,
                            },
                        ),
                    }
                }
            }
            WorkerCommand::Shutdown => {
                write_envelope(
                    &mut stdout,
                    &WorkerEnvelope::reply(&request.id, WorkerEvent::Shutdown),
                )?;
                return Ok(());
            }
        };
        write_envelope(&mut stdout, &envelope)?;
    }
    Ok(())
}

fn write_envelope(stdout: &mut impl Write, envelope: &WorkerEnvelope) -> io::Result<()> {
    let encoded = encode_line(envelope)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    writeln!(stdout, "{encoded}")?;
    stdout.flush()
}
