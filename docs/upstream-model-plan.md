# Upstream model-runtime plan

Lessons from [hyperfield/ai-file-sorter](https://github.com/hyperfield/ai-file-sorter)
(Qt 0.9.0–1.9.2) that are still worth taking, in this fork’s process-isolated
shape. Port the **behaviour**, not `LocalLLMClient.cpp`.

Companion to [`golden-path-execution-plan.md`](golden-path-execution-plan.md)
(workspace UX) and [`protocol.md`](protocol.md) (JSONL contracts). This file is
the llama / catalog download sequence only.

## Constraints that do not move

- The UI process never loads llama.cpp. Only `aifs-worker-llm` does.
- Catalog downloads stay SHA-256 verified. GGUF magic is a cheap extra probe
  for custom paths, not a replacement for the digest.
- Default `cargo test --workspace` must not compile llama.cpp. Llama work is
  `cargo test -p aifs-worker-llm --features llama` (CI `llama-cpu` job).
- Prompts stay JSON-shaped. Do not port the Qt “Main : Sub” sanitizer or
  temperature / min_p sampling; greedy decode is the right default for extract.
- Scan logs `fallback` on CPU retry. No GUI modal.
- Microsoft Store / CUDA packaging stays parked with the golden path until that
  path is solid.

## Already taken (wave 0)

Landed with the load-recovery work. Do not re-port these from Qt:

| Upstream lesson | This fork |
|-----------------|-----------|
| GGUF magic before load | `has_gguf_header` on custom paths; catalog still SHA-256 |
| Metal as `MTL` (1.9.2) | `gpu_preference` accepts `metal` / `mtl` |
| Windows CUDA / Vulkan probes | `nvcuda.dll` / `nvml.dll` / `vulkan-1.dll` plus Linux `/dev/nvidia0` and `/dev/dri` |
| Fewer GPU layers before CPU | GGUF `block_count`; full → ~75% → ~50%, then CPU |
| Context allocation fallbacks | `AIFS_CTX_TOKENS` (default 4096) then 2048 / 1024 / 512 |
| Infer GPU OOM | One CPU reload + retry for generate and describe |
| Prompt shrink | `document.text` **and** `description` |

Isolation, llama.cpp chat templates, libmtmd JPEG/PNG/WebP, hosted describe
without pixels, and describe-before-categorize are already stronger than Qt
in-process llama. Keep them.

## Sequence

Waves are ordered by user pain and dependency, not calendar. Waves 1–4 can
land independently of each other after wave 0; do them in this order unless a
later wave is blocking a golden-path phase.

### Wave 1 — Resume catalog downloads (landed)

**Why it matters.** Gemma 3 4B Q4 is ~2.5 GiB (`GEMMA_TEXT_BYTES`).

**Done.** `.part` is kept on cancel; HTTP `Range` resumes when the server
answers 206; a `200` (ignored Range) or `416` restarts from byte 0;
transport / 5xx / 429 keep `.part`. SHA-256 still gates the finished file.
See `crates/aifs-engine/src/download.rs`.

### Wave 2 — Fit prompts to the real context window (landed)

**Done.** Char shrink of `document.text` / `description` still runs first.
If the tokenized prompt is still over `n_ctx - max_tokens`, oldest **user**
tokens are dropped and the system JSON/schema turn is kept. Image describe
shortens the user text (not image tokens). See `prompt.rs` /
`oldest_user_drop_start`.

### Wave 3 — Cheaper first `n_gpu_layers` guess (landed)

**Why it matters.** Wave 0 only steps down **after** a failed load. Each GPU
attempt reloads ~2.5 GiB. Upstream estimates free VRAM (CUDA / Metal / Vulkan)
and caps integrated GPUs so the first `ngl` is already plausible.

**Done.** `gpu_layer_load_attempts` still walks full → ~75% → ~50% (or
999 → 32 → 16 when `block_count` is unknown). A best-effort free-VRAM probe
(`nvidia-smi` MiB CSV, then Linux `mem_info_vram_free`) caps the first attempt
at ~80 MiB/layer. Zero free VRAM skips GPU and loads CPU. Missing probe leaves
the wave 0 ladder unchanged. Explicit `n_gpu_layers` / `AIFS_N_GPU_LAYERS` is
still tried once, then CPU. `loaded.fallback` may include
`reduced-ngl from free VRAM probe` when the first attempt is below the uncapped
first. See `bins/aifs-worker-llm/src/device.rs`.

### Wave 4 — Cancel scan during infer (landed)

**Why it matters.** Engine `cancel` is a stdin flag checked **between files**.
`aifs-worker-client` waits up to 120s for `categorize` / `describe`. The llama
decode loop is not cooperative. A user who hits Cancel during a local infer
waits out that timeout (or the remaining tokens). Golden-path Phase 6/7 wants
cancel that feels immediate.

**Done.** `categorize_while` / `describe_while` poll scan cancel every 100ms
and kill the LLM child when it flips. The in-flight file is not persisted;
scan logs `analysis stopped` and returns `WorkStatus::Cancelled`. Wait slices
still fire every 15s so engine idle 180s does not trip on a healthy infer.
Hosted HTTP mid-body cancel is unchanged. See
`crates/aifs-worker-client` and `crates/aifs-engine/src/analyze.rs`.

### Wave 5 — Packaged ggml (when we ship a llama sidecar)

**Why it matters.** Upstream 1.9.1 builds Windows/Linux ggml with
`GGML_NATIVE=OFF` / SSE4.2 so packaged CPUs without AVX2 still run. macOS
1.7.3 sets rpath and refuses Homebrew `libggml`. This fork’s isolation helps,
but a shipped `aifs-worker-llm` still dlopens ggml next to itself.

**Do**

- When (and only when) desktop packaging embeds the llama-enabled worker:
  cmake `GGML_NATIVE=OFF` for Windows and Linux CPU wheels; document the
  generator flags next to `pnpm llama`.
- On macOS packages, rpath the bundled ggml and do not fall back to Homebrew.

**Do not**

- Change default CI or `cargo test --workspace` to native-tune or to compile
  llama.
- Un-park the Microsoft Store / CUDA matrix in the golden path just to land
  this. Wave 5 is packaging, not scan UX.

**Depends on.** A decision to ship llama in the Tauri bundle. Until then,
leave cmake at llama-cpp-2 defaults for local `pnpm llama` builds.

## Out of scope (will not take from upstream)

| Upstream | Why we skip |
|----------|-------------|
| Extra catalog models (Mistral 7B, Gemma 1.1 7B, LLaVA, Llama 3B) | Slim catalog is intentional; add only if Gemma-only is a product gap |
| Vendored llama.cpp pin `4f31eed` | Stay on crates.io `llama-cpp-2` until mtmd/API breaks |
| Temperature / min_p sampling | Greedy JSON extract |
| CPU-fallback GUI modal | Scan-log `fallback` is the protocol choice |
| Qt writable CA bundle | rustls + webpki-roots |
| Learning-from-reviews as a silent recategorizer | Golden-path “out of scope until the path is solid” |
| Category language / branching whitelist | Golden-path Phase 4, not llama runtime |

## Mapping to the golden path

| Golden-path phase | This plan |
|-------------------|-----------|
| Phase 5 Setup (download, hash, shared storage) | Wave 1 resume is the remaining Setup hole |
| Phase 6 real analysis workers | Waves 2–4 (prompt fit, ngl guess, infer cancel) |
| Phase 7 cancel / resume at tree scale | Wave 4 plus existing scan checkpoints |
| Parked: Store / CUDA matrix | Wave 5 only when packaging starts |

## Done when

- Wave 1: a cancelled 2.5 GiB fetch can continue without re-downloading
  verified prefix bytes; checksum failures still delete junk.
- Wave 2: a huge document/description still yields JSON (or a logged skip),
  not a context-window hard fail, after char shrink is exhausted.
- Wave 3: modest GPUs attempt a reduced `ngl` first when a probe exists;
  missing probes still follow the wave 0 ladder.
- Wave 4: Cancel during local categorize returns on the order of process
  kill, not `INFER_TIMEOUT`.
- Wave 5: packaged CPU llama starts on SSE4.2-only Windows/Linux; macOS
  packages do not load Homebrew ggml.
