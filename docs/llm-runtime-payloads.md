# LLM runtime payloads

Each llama.cpp accelerator is a **directory**, not a feature flag on a shared
`aifs-worker-llm` sidecar. The UI never loads llama.cpp; the engine will spawn
the worker that lives inside the chosen folder (staging and autoselect are later
work). Completeness is defined here so packaging and discovery cannot drift.

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

See `crates/aifs-protocol/src/llm_payload.rs`.
