//! Isolated LLM worker stub. File extraction is not a language-model job.

use aifs_protocol::worker::WorkerKind;

fn main() {
    if let Err(error) = aifs_worker_runtime::run_stdio(WorkerKind::Llm, &["stub"], |_, _| {
        Err("llm worker does not extract files; chat tools stay in the engine".to_owned())
    }) {
        eprintln!("aifs-worker-llm: {error}");
        std::process::exit(1);
    }
}
