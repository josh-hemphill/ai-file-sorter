//! Isolated LLM worker. Stub infer until the llama.cpp backend is compiled in.

mod device;
mod stub;

use aifs_protocol::worker::WorkerKind;
use stub::StubHandler;

fn main() {
    if let Err(error) = aifs_worker_runtime::run_with_handler(
        WorkerKind::Llm,
        &["stub", "load", "unload", "categorize", "describe", "chat"],
        StubHandler::default(),
    ) {
        eprintln!("aifs-worker-llm: {error}");
        std::process::exit(1);
    }
}
