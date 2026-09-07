//! Dispatches stub infer, optional llama.cpp, and hosted HTTP backends.

use crate::hosted::HostedHandler;
use crate::stub::StubHandler;
use aifs_domain::{Evidence, ObservedEntry};
use aifs_protocol::{ModelBackend, is_hosted_backend};
use aifs_worker_runtime::{LoadedModel, WorkerHandler};
use std::path::Path;

#[cfg(feature = "llama")]
use crate::gguf::is_local_gguf;
#[cfg(feature = "llama")]
use crate::llama::LlamaHandler;

#[derive(Default)]
enum Active {
    #[default]
    None,
    Stub,
    Hosted,
    #[cfg(feature = "llama")]
    Llama,
}

/// Session-lived LLM worker. Default builds use the stub; `--features llama` loads GGUFs.
#[derive(Default)]
pub struct LlmHandler {
    stub: StubHandler,
    hosted: HostedHandler,
    #[cfg(feature = "llama")]
    llama: LlamaHandler,
    active: Active,
}

impl WorkerHandler for LlmHandler {
    fn extract(
        &mut self,
        _root: &Path,
        _entry: &ObservedEntry,
    ) -> Result<Option<Evidence>, String> {
        Err("llm worker does not extract files".to_owned())
    }

    fn load(
        &mut self,
        backend: ModelBackend,
        gpu_preference: &str,
        n_gpu_layers: Option<u32>,
        api_key: Option<String>,
        storage_dir: &str,
    ) -> Result<LoadedModel, String> {
        if matches!(backend, ModelBackend::Off) {
            return Err("cannot load an off slot".to_owned());
        }
        if is_hosted_backend(&backend) {
            let _ = self.stub.unload();
            #[cfg(feature = "llama")]
            {
                let _ = self.llama.unload();
            }
            let loaded =
                self.hosted
                    .load(backend, gpu_preference, n_gpu_layers, api_key, storage_dir)?;
            self.active = Active::Hosted;
            return Ok(loaded);
        }
        let _ = self.hosted.unload();
        #[cfg(feature = "llama")]
        if is_local_gguf(&backend) {
            let _ = self.stub.unload();
            let loaded =
                self.llama
                    .load(backend, gpu_preference, n_gpu_layers, api_key, storage_dir)?;
            self.active = Active::Llama;
            return Ok(loaded);
        }
        #[cfg(feature = "llama")]
        {
            let _ = self.llama.unload();
        }
        let loaded = self
            .stub
            .load(backend, gpu_preference, n_gpu_layers, api_key, storage_dir)?;
        self.active = Active::Stub;
        Ok(loaded)
    }

    fn unload(&mut self) -> Result<(), String> {
        match self.active {
            #[cfg(feature = "llama")]
            Active::Llama => self.llama.unload()?,
            Active::Hosted => self.hosted.unload()?,
            Active::Stub | Active::None => self.stub.unload()?,
        }
        self.active = Active::None;
        Ok(())
    }

    fn categorize(
        &mut self,
        root: &Path,
        entry: &ObservedEntry,
        evidence: &[Evidence],
    ) -> Result<Option<Evidence>, String> {
        match self.active {
            #[cfg(feature = "llama")]
            Active::Llama => self.llama.categorize(root, entry, evidence),
            Active::Hosted => self.hosted.categorize(root, entry, evidence),
            Active::Stub => self.stub.categorize(root, entry, evidence),
            Active::None => Err("load a model before infer".to_owned()),
        }
    }

    fn describe(
        &mut self,
        root: &Path,
        entry: &ObservedEntry,
        evidence: &[Evidence],
    ) -> Result<Option<Evidence>, String> {
        match self.active {
            #[cfg(feature = "llama")]
            Active::Llama => self.llama.describe(root, entry, evidence),
            Active::Hosted => self.hosted.describe(root, entry, evidence),
            Active::Stub => self.stub.describe(root, entry, evidence),
            Active::None => Err("load a model before infer".to_owned()),
        }
    }

    fn chat(&mut self, utterance: &str, context: &str) -> Result<String, String> {
        match self.active {
            #[cfg(feature = "llama")]
            Active::Llama => self.llama.chat(utterance, context),
            Active::Hosted => self.hosted.chat(utterance, context),
            Active::Stub => self.stub.chat(utterance, context),
            Active::None => Err("load a model before infer".to_owned()),
        }
    }
}
