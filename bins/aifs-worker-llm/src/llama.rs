//! llama.cpp backend. Compiled only with `--features llama`.

use crate::device::{cpu_retry_plan, requested_n_gpu_layers, resolve_device};
use crate::gguf::{GgufFiles, resolve_gguf};
use crate::parse::{apply_parsed, parse_infer_json};
use crate::prompt::{
    CATEGORIZE_SYSTEM, CHAT_SYSTEM, DESCRIBE_SYSTEM, categorize_user, describe_user,
};
use crate::vision::{PixelPlan, pixel_plan};
use aifs_domain::{Confidence, EntryKind, Evidence, EvidenceSource, FileFamily, ObservedEntry};
use aifs_protocol::ModelBackend;
use aifs_worker_runtime::{LoadedModel, WorkerHandler};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::mtmd::{
    MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText, mtmd_default_marker,
};
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

const N_CTX: u32 = 4096;
const MAX_GEN_TOKENS: i32 = 128;
const CHAT_GEN_TOKENS: i32 = 512;
const BATCH_FLOOR: usize = 512;
const ALL_GPU_LAYERS: u32 = 999;
const LLAMA_CONFIDENCE: f32 = 0.55;

/// Session-lived llama.cpp backend. One GGUF is loaded at a time.
#[derive(Default)]
pub struct LlamaHandler {
    backend: Option<LlamaBackend>,
    loaded: Option<LoadedGguf>,
}

struct LoadedGguf {
    model: LlamaModel,
    mtmd: Option<MtmdContext>,
    weights: PathBuf,
    info: LoadedModel,
}

enum DescribePrompt {
    Pixels { user: String, image: PathBuf },
    Text { user: String },
}

impl WorkerHandler for LlamaHandler {
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
        let _ = api_key;
        if matches!(backend, ModelBackend::Off) {
            return Err("cannot load an off slot".to_owned());
        }
        let files = resolve_gguf(&backend, storage_dir)?;
        let (device, fallback) = resolve_device(gpu_preference);
        let n_gpu_layers = gpu_layers(&device, requested_n_gpu_layers(n_gpu_layers));
        if self.loaded.as_ref().is_some_and(|loaded| {
            loaded.weights == files.weights
                && loaded.info.device == device
                && loaded.info.n_gpu_layers == n_gpu_layers
        }) {
            return self.reuse_loaded(&files, device, n_gpu_layers, fallback);
        }
        match self.try_load_weights(&files, device.clone(), n_gpu_layers, fallback.clone()) {
            Ok(info) => Ok(info),
            Err(error) => match cpu_retry_plan(&device, fallback, &error) {
                Some((cpu, cpu_layers, cpu_fallback)) => {
                    self.try_load_weights(&files, cpu, cpu_layers, cpu_fallback)
                }
                None => Err(error),
            },
        }
    }

    fn unload(&mut self) -> Result<(), String> {
        self.loaded = None;
        Ok(())
    }

    fn categorize(
        &mut self,
        _root: &Path,
        entry: &ObservedEntry,
        evidence: &[Evidence],
    ) -> Result<Option<Evidence>, String> {
        let model_id = self.require_loaded()?.info.model.clone();
        if entry.kind != EntryKind::File {
            return Ok(None);
        }
        let text = self.complete(
            CATEGORIZE_SYSTEM,
            &categorize_user(entry, evidence),
            MAX_GEN_TOKENS,
        )?;
        Ok(evidence_from_text(&model_id, entry, &text, true))
    }

    fn describe(
        &mut self,
        root: &Path,
        entry: &ObservedEntry,
        evidence: &[Evidence],
    ) -> Result<Option<Evidence>, String> {
        let model_id = self.require_loaded()?.info.model.clone();
        if entry.kind != EntryKind::File {
            return Ok(None);
        }
        if !matches!(entry.family, FileFamily::Image | FileFamily::RawImage) {
            return Ok(None);
        }
        let text = match self.describe_prompt(root, entry, evidence) {
            DescribePrompt::Pixels { user, image } => self
                .complete_with_image(DESCRIBE_SYSTEM, &user, &image, MAX_GEN_TOKENS)
                .or_else(|_| self.complete(DESCRIBE_SYSTEM, &user, MAX_GEN_TOKENS))?,
            DescribePrompt::Text { user } => {
                self.complete(DESCRIBE_SYSTEM, &user, MAX_GEN_TOKENS)?
            }
        };
        Ok(evidence_from_text(&model_id, entry, &text, false))
    }

    fn chat(&mut self, utterance: &str, context: &str) -> Result<String, String> {
        let _ = self.require_loaded()?;
        let user = if context.trim().is_empty() {
            utterance.to_owned()
        } else {
            format!("{utterance}\n\nContext:\n{context}")
        };
        self.complete(CHAT_SYSTEM, &user, CHAT_GEN_TOKENS)
    }
}

impl LlamaHandler {
    fn ensure_backend(&mut self) -> Result<&LlamaBackend, String> {
        if self.backend.is_none() {
            let mut backend = LlamaBackend::init().map_err(|error| error.to_string())?;
            backend.void_logs();
            self.backend = Some(backend);
        }
        self.backend
            .as_ref()
            .ok_or_else(|| "llama backend missing".to_owned())
    }

    fn require_loaded(&self) -> Result<&LoadedGguf, String> {
        self.loaded
            .as_ref()
            .ok_or_else(|| "load a model before infer".to_owned())
    }

    fn try_load_weights(
        &mut self,
        files: &GgufFiles,
        device: String,
        n_gpu_layers: u32,
        fallback: Option<String>,
    ) -> Result<LoadedModel, String> {
        self.loaded = None;
        let llama = self.ensure_backend()?;
        let params = LlamaModelParams::default().with_n_gpu_layers(n_gpu_layers);
        let model = LlamaModel::load_from_file(llama, &files.weights, &params)
            .map_err(|error| error.to_string())?;
        let mut info = LoadedModel {
            device,
            model: files.label.clone(),
            n_gpu_layers,
            fallback,
        };
        let mtmd = match &files.mmproj {
            Some(path) => match init_mtmd(&model, path, info.device != "cpu") {
                Ok(ctx) => Some(ctx),
                Err(error) => {
                    append_fallback(
                        &mut info.fallback,
                        &format!(
                            "mmproj failed to load ({error}); describe uses filename and EXIF"
                        ),
                    );
                    None
                }
            },
            None => None,
        };
        self.loaded = Some(LoadedGguf {
            model,
            mtmd,
            weights: files.weights.clone(),
            info: info.clone(),
        });
        Ok(info)
    }

    fn reuse_loaded(
        &mut self,
        files: &GgufFiles,
        device: String,
        n_gpu_layers: u32,
        fallback: Option<String>,
    ) -> Result<LoadedModel, String> {
        let loaded = self
            .loaded
            .as_mut()
            .ok_or_else(|| "load a model before infer".to_owned())?;
        loaded.info.device = device;
        loaded.info.model = files.label.clone();
        loaded.info.n_gpu_layers = n_gpu_layers;
        loaded.info.fallback = fallback;
        Ok(loaded.info.clone())
    }

    fn complete(&mut self, system: &str, user: &str, max_tokens: i32) -> Result<String, String> {
        let llama = self
            .backend
            .as_ref()
            .ok_or_else(|| "load a model before infer".to_owned())?;
        let loaded = self
            .loaded
            .as_ref()
            .ok_or_else(|| "load a model before infer".to_owned())?;
        generate(llama, &loaded.model, system, user, max_tokens)
    }

    fn describe_prompt(
        &self,
        root: &Path,
        entry: &ObservedEntry,
        evidence: &[Evidence],
    ) -> DescribePrompt {
        let user = describe_user(entry, evidence);
        let image = root.join(entry.path.as_str());
        let has_mtmd = self
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.mtmd.is_some());
        match pixel_plan(&image, entry.family) {
            PixelPlan::Attach if has_mtmd => DescribePrompt::Pixels { user, image },
            PixelPlan::Attach | PixelPlan::TextOnly { .. } => DescribePrompt::Text { user },
        }
    }

    fn complete_with_image(
        &mut self,
        system: &str,
        user: &str,
        image: &Path,
        max_tokens: i32,
    ) -> Result<String, String> {
        let llama = self
            .backend
            .as_ref()
            .ok_or_else(|| "load a model before infer".to_owned())?;
        let loaded = self
            .loaded
            .as_ref()
            .ok_or_else(|| "load a model before infer".to_owned())?;
        let mtmd = loaded
            .mtmd
            .as_ref()
            .ok_or_else(|| "mmproj is not loaded".to_owned())?;
        generate_with_image(llama, &loaded.model, mtmd, system, user, image, max_tokens)
    }
}

fn gpu_layers(device: &str, requested: Option<u32>) -> u32 {
    if device == "cpu" {
        0
    } else {
        requested.unwrap_or(ALL_GPU_LAYERS)
    }
}

fn append_fallback(fallback: &mut Option<String>, note: &str) {
    *fallback = Some(match fallback.take() {
        Some(existing) => format!("{existing} {note}"),
        None => note.to_owned(),
    });
}

fn init_mtmd(model: &LlamaModel, mmproj: &Path, use_gpu: bool) -> Result<MtmdContext, String> {
    let path = mmproj
        .to_str()
        .ok_or_else(|| format!("{} is not valid UTF-8", mmproj.display()))?;
    let n_threads = i32::try_from(
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(4),
    )
    .unwrap_or(4);
    let params = MtmdContextParams {
        use_gpu,
        print_timings: false,
        n_threads,
        ..MtmdContextParams::default()
    };
    MtmdContext::init_from_file(path, model, &params).map_err(|error| error.to_string())
}

fn evidence_from_text(
    model: &str,
    entry: &ObservedEntry,
    text: &str,
    want_category: bool,
) -> Option<Evidence> {
    let parsed = parse_infer_json(text)?;
    if want_category && parsed.category.is_none() {
        return None;
    }
    if !want_category && parsed.description.is_none() {
        return None;
    }
    let mut bag = Evidence::new(
        entry.id,
        EvidenceSource::LocalModel {
            model: model.to_owned(),
        },
        Confidence::new(LLAMA_CONFIDENCE),
    );
    apply_parsed(&parsed, |key, value| {
        bag.facts.insert(key.to_owned(), value.to_owned());
    });
    Some(bag)
}

fn generate(
    backend: &LlamaBackend,
    model: &LlamaModel,
    system: &str,
    user: &str,
    max_tokens: i32,
) -> Result<String, String> {
    let prompt = chat_prompt(model, system, user);
    let n_ctx = NonZeroU32::new(N_CTX).unwrap_or(NonZeroU32::MIN);
    let ctx_params = LlamaContextParams::default().with_n_ctx(Some(n_ctx));
    let mut ctx = model
        .new_context(backend, ctx_params)
        .map_err(|error| error.to_string())?;
    let tokens = model
        .str_to_token(&prompt, AddBos::Always)
        .map_err(|error| error.to_string())?;
    if tokens.is_empty() {
        return Err("prompt produced no tokens".to_owned());
    }
    let n_ctx_i32 = i32::try_from(N_CTX).unwrap_or(i32::MAX);
    let n_prompt = i32::try_from(tokens.len()).unwrap_or(i32::MAX);
    if n_prompt.saturating_add(max_tokens) > n_ctx_i32 {
        return Err("prompt exceeds the llama.cpp context window".to_owned());
    }
    let batch_size = tokens.len().max(BATCH_FLOOR);
    let mut batch = LlamaBatch::new(batch_size, 1);
    let last_index = i32::try_from(tokens.len().saturating_sub(1)).unwrap_or(0);
    for (i, token) in (0_i32..).zip(tokens) {
        batch
            .add(token, i, &[0], i == last_index)
            .map_err(|error| error.to_string())?;
    }
    ctx.decode(&mut batch).map_err(|error| error.to_string())?;

    let mut sampler = LlamaSampler::greedy();
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut output = String::new();
    let max = usize::try_from(max_tokens).unwrap_or(0);
    for n_cur in (batch.n_tokens()..).take(max) {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);
        if model.is_eog_token(token) {
            break;
        }
        if let Ok(piece) = model.token_to_piece(token, &mut decoder, true, None) {
            output.push_str(&piece);
        }
        batch.clear();
        batch
            .add(token, n_cur, &[0], true)
            .map_err(|error| error.to_string())?;
        ctx.decode(&mut batch).map_err(|error| error.to_string())?;
    }
    Ok(output)
}

fn generate_with_image(
    backend: &LlamaBackend,
    model: &LlamaModel,
    mtmd: &MtmdContext,
    system: &str,
    user: &str,
    image: &Path,
    max_tokens: i32,
) -> Result<String, String> {
    if !mtmd.support_vision() {
        return Err("mmproj does not support vision".to_owned());
    }
    let image_path = image
        .to_str()
        .ok_or_else(|| format!("{} is not valid UTF-8", image.display()))?;
    let marker = mtmd_default_marker();
    let user = format!("{user}\n{marker}");
    let prompt = chat_prompt(model, system, &user);
    let bitmap =
        MtmdBitmap::from_file(mtmd, image_path, false).map_err(|error| error.to_string())?;
    if bitmap.is_audio() {
        return Err("image path decoded as audio".to_owned());
    }
    let chunks = mtmd
        .tokenize(
            MtmdInputText {
                text: prompt,
                add_special: true,
                parse_special: true,
            },
            &[&bitmap],
        )
        .map_err(|error| error.to_string())?;
    let n_prompt = chunks.total_tokens();
    let n_ctx = usize::try_from(N_CTX).unwrap_or(0);
    let n_gen = usize::try_from(max_tokens).unwrap_or(0);
    if n_prompt.saturating_add(n_gen) > n_ctx {
        return Err("prompt exceeds the llama.cpp context window".to_owned());
    }
    let n_ctx_nz = NonZeroU32::new(N_CTX).unwrap_or(NonZeroU32::MIN);
    let ctx_params = LlamaContextParams::default().with_n_ctx(Some(n_ctx_nz));
    let mut ctx = model
        .new_context(backend, ctx_params)
        .map_err(|error| error.to_string())?;
    let n_batch = i32::try_from(n_prompt.max(BATCH_FLOOR)).unwrap_or(i32::MAX);
    let n_past = chunks
        .eval_chunks(mtmd, &ctx, 0, 0, n_batch, true)
        .map_err(|error| error.to_string())?;
    sample_continuation(&mut ctx, model, n_past, max_tokens)
}

fn sample_continuation(
    ctx: &mut llama_cpp_2::context::LlamaContext,
    model: &LlamaModel,
    mut n_cur: i32,
    max_tokens: i32,
) -> Result<String, String> {
    let mut sampler = LlamaSampler::greedy();
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut output = String::new();
    let mut batch = LlamaBatch::new(BATCH_FLOOR, 1);
    let max = usize::try_from(max_tokens).unwrap_or(0);
    for round in 0..max {
        let idx = if round == 0 { -1 } else { batch.n_tokens() - 1 };
        let token = sampler.sample(ctx, idx);
        sampler.accept(token);
        if model.is_eog_token(token) {
            break;
        }
        if let Ok(piece) = model.token_to_piece(token, &mut decoder, true, None) {
            output.push_str(&piece);
        }
        batch.clear();
        batch
            .add(token, n_cur, &[0], true)
            .map_err(|error| error.to_string())?;
        ctx.decode(&mut batch).map_err(|error| error.to_string())?;
        n_cur = n_cur.saturating_add(1);
    }
    Ok(output)
}

fn chat_prompt(model: &LlamaModel, system: &str, user: &str) -> String {
    let fallback = format!("{system}\n\nUser:\n{user}\n\nAssistant:\n");
    let Ok(template) = model.chat_template(None) else {
        return fallback;
    };
    let Ok(system_msg) = LlamaChatMessage::new("system".to_owned(), system.to_owned()) else {
        return fallback;
    };
    let Ok(user_msg) = LlamaChatMessage::new("user".to_owned(), user.to_owned()) else {
        return fallback;
    };
    model
        .apply_chat_template(&template, &[system_msg, user_msg], true)
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_device_always_offloads_zero_layers() {
        assert_eq!(gpu_layers("cpu", Some(99)), 0);
        assert_eq!(gpu_layers("cpu", None), 0);
    }

    #[test]
    fn accelerator_uses_explicit_or_all_layers() {
        assert_eq!(gpu_layers("cuda", Some(32)), 32);
        assert_eq!(gpu_layers("cuda", None), ALL_GPU_LAYERS);
        assert_eq!(gpu_layers("vulkan", Some(0)), 0);
    }
}
