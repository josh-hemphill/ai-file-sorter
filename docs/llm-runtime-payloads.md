# LLM runtime payloads

Each llama.cpp accelerator is a **directory**, not a feature flag on a shared
`aifs-worker-llm` sidecar. The UI never loads llama.cpp; the engine will spawn
the worker that lives inside the chosen folder (autoselect is later work).
Completeness is defined here so packaging and discovery cannot drift.

## Layout

```text
llm-runtime/
  cpu/      aifs-worker-llm + llama + core ggml
  cuda/     same, plus ggml-cuda (and CUDA runtime libs when staged)
  vulkan/   same, plus ggml-vulkan
  metal/    macOS; same core libs as cpu
```

Path helper: `llm_payload_dir(root, accel)` → `root/llm-runtime/<accel>`.

Auto-select order (host probes applied later): CUDA, Vulkan, Metal, CPU
(`LLM_ACCEL_AUTO_ORDER`).

## Completeness (`payload_complete`)

A directory is complete for an accelerator when:

1. `aifs-worker-llm` (or the Tauri `{stem}-{triple}` name) is a **non-empty** file.
2. Every required native-lib prefix is present as `.dll` / `.so` / `.dylib`:

| Accel | Required prefixes |
|-------|-------------------|
| cpu, metal | `llama`, `ggml` (core: `ggml`, `ggml-base`, or `ggml-cpu` only) |
| cuda | those, plus `ggml-cuda` |
| vulkan | those, plus `ggml-vulkan` |

`nvcuda.dll` / `libcuda.so` are host driver libraries. They never satisfy
`ggml-cuda` or core `ggml`. `ggml-cuda.dll` does not count as core `ggml`.

`infer_accel_from_libs` returns CUDA if the CUDA payload is complete, else
Vulkan, else CPU. Metal is not inferred from library names.

## Staging

After a real `aifs-worker-llm` copy, `src-tauri` snapshots the worker and its
runtime libs into:

- `target/{profile}/llm-runtime/<accel>/`
- `apps/desktop/src-tauri/resources/llm-runtime/<accel>/`

`<accel>` is inferred from libraries next to the Cargo binary (and `deps/`),
including `ggml-cuda` / `ggml-vulkan` even when those plugins are not yet a
complete payload directory. CUDA toolkit libs (`CUDA_PATH/bin`) are copied only
into a CUDA payload. Staging one accelerator does not delete sibling folders
(`cpu` must not wipe `cuda`). Sidecar copies into `binaries/` stay flat until
spawn uses the payload directory. Tauri bundles `resources/llm-runtime` as
the directory `llm-runtime` (a glob map would flatten `cuda/ggml.dll` and
`cpu/ggml.dll` onto the same filename).

See `crates/aifs-protocol/src/llm_payload.rs` and
`apps/desktop/src-tauri/sidecar_copy.rs`.
