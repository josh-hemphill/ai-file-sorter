//! Cooperative cancel for in-flight engine requests.
//!
//! Stdio reads `cancel` on a background thread so a long `scan` / `apply` can notice
//! the flag between files. The UI process still never opens user files.

use aifs_protocol::RequestId;
use std::sync::{Arc, Mutex};

/// Shared cancel flag between the stdin reader and request handler.
#[derive(Debug, Default)]
pub struct CancelGate {
    in_flight: Mutex<Option<RequestId>>,
    cancelled: Mutex<Option<RequestId>>,
}

/// Result of a cancellable stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkStatus {
    /// Stage finished normally.
    Completed,
    /// Stage stopped because [`CancelGate::is_cancelled`] became true.
    Cancelled,
}

impl CancelGate {
    /// Marks `id` as the request currently being handled.
    pub fn begin(&self, id: RequestId) {
        if let Ok(mut in_flight) = self.in_flight.lock() {
            *in_flight = Some(id);
        }
    }

    /// Clears in-flight / cancel state for `id` when that request ends.
    pub fn end(&self, id: &RequestId) {
        if let Ok(mut in_flight) = self.in_flight.lock()
            && in_flight.as_ref() == Some(id)
        {
            *in_flight = None;
        }
        if let Ok(mut cancelled) = self.cancelled.lock()
            && cancelled.as_ref() == Some(id)
        {
            *cancelled = None;
        }
    }

    /// Records that `target` should stop at the next cooperative check.
    pub fn request_cancel(&self, target: RequestId) {
        if let Ok(mut cancelled) = self.cancelled.lock() {
            *cancelled = Some(target);
        }
    }

    /// True when `id` has been asked to stop.
    pub fn is_cancelled(&self, id: &RequestId) -> bool {
        self.cancelled
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .as_ref()
            == Some(id)
    }
}

/// RAII arming of [`CancelGate::begin`] / [`end`].
pub struct CancelScope {
    gate: Arc<CancelGate>,
    id: RequestId,
}

impl CancelScope {
    /// Arms the gate for `id`.
    pub fn new(gate: Arc<CancelGate>, id: RequestId) -> Self {
        gate.begin(id.clone());
        Self { gate, id }
    }
}

impl Drop for CancelScope {
    fn drop(&mut self) {
        self.gate.end(&self.id);
    }
}
