//! Isolated LLM worker. Default infer is stubbed; `--features llama` loads GGUFs via llama.cpp.

mod device;
#[cfg(any(test, feature = "llama"))]
mod gguf;
mod handler;
#[cfg(feature = "llama")]
mod llama;
#[cfg(any(test, feature = "llama"))]
mod parse;
#[cfg(any(test, feature = "llama"))]
mod prompt;
mod stub;

use aifs_protocol::worker::WorkerKind;
use handler::LlmHandler;

fn main() {
    let caps = capabilities();
    if let Err(error) =
        aifs_worker_runtime::run_with_handler(WorkerKind::Llm, &caps, LlmHandler::default())
    {
        eprintln!("aifs-worker-llm: {error}");
        std::process::exit(1);
    }
}

fn capabilities() -> Vec<&'static str> {
    let mut caps = vec!["load", "unload", "categorize", "describe", "chat"];
    #[cfg(feature = "llama")]
    caps.push("llama");
    #[cfg(not(feature = "llama"))]
    caps.push("stub");
    #[cfg(feature = "cuda")]
    caps.push("cuda");
    #[cfg(feature = "vulkan")]
    caps.push("vulkan");
    #[cfg(feature = "metal")]
    caps.push("metal");
    caps
}
