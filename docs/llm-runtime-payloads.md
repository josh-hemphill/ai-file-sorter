# LLM runtime payloads

Each llama.cpp accelerator is a **directory**, not a feature flag on a shared
`aifs-worker-llm` sidecar. The UI never loads llama.cpp; the engine will spawn
the worker that lives inside the chosen folder. Completeness is defined here so
packaging and discovery cannot drift.

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
| cuda | those, plus `ggml-cuda` **or** a statically linked worker that imports `cublas`/`cudart` with those toolkit DLLs beside it |
| vulkan | those, plus `ggml-vulkan` |

MSVC `llama-cpp-sys-2` often sets `BUILD_SHARED_LIBS=OFF`, so there is no
`llama.dll` / `ggml-cuda.dll`. CUDA 13 keeps `cublas64_*.dll` in
`CUDA_PATH/bin/x64` (not `bin/`). Staging copies that toolkit slice next to
the worker and does not copy `nvcuda.dll`.

`nvcuda.dll` / `libcuda.so` are host driver libraries. They never satisfy
`ggml-cuda` or core `ggml`. `ggml-cuda.dll` does not count as core `ggml`.

`infer_accel_from_libs` returns CUDA if the CUDA payload is complete, else
Vulkan, else CPU. Metal is not inferred from library names.

## Staging

After a real `aifs-worker-llm` copy, `src-tauri` snapshots the worker and its
runtime libs into:

- `target/{profile}/llm-runtime/<accel>/`
- `apps/desktop/src-tauri/resources/llm-runtime/<accel>/`

`<accel>` is inferred from libraries next to the Cargo binary, in `deps/`, and
nested under `build/llama-cpp-*/out` (MSVC cmake-rs often leaves
`ggml-cuda.dll` in `out/bin/Release` or `out/build/bin/Release`, not the `out`
root). Staging walks those trees and skips `CMakeFiles`. `ggml-cuda` /
`ggml-vulkan` still count even when those plugins are not yet a complete
payload directory. CUDA toolkit libs (`CUDA_PATH/bin`) are copied only
into a CUDA payload. `pnpm llama:cuda` (`AIFS_LLM_FEATURES=cuda`) fails
closed if `ggml-cuda` was not harvested, instead of writing a worker-only
`llm-runtime/cpu`. Staging one accelerator does not delete sibling folders
(`cpu` must not wipe `cuda`). Extract workers still copy into `binaries/` as
`externalBin` sidecars. The LLM worker is **not** listed in `externalBin` and
is not copied into `binaries/`; a packaged flat sidecar would collide with
nested payloads. Tauri bundles `resources/llm-runtime` as
the directory `llm-runtime` (a glob map would flatten `cuda/ggml.dll` and
`cpu/ggml.dll` onto the same filename).
Staging sets Unix execute bits on the payload worker (resource copies can drop `+x`).

## Discovery and spawn

`list_payloads_under(root)` returns complete `llm-runtime/<accel>/` folders.
The engine also walks ancestors of the engine exe and `CARGO_MANIFEST_DIR`
(`resources/`, `target/{debug,release}`). Packaged macOS also walks
`Contents/Resources` beside `Contents/MacOS`. `select_llm_payload` uses
`AIFS_LLM_BACKEND` when set, else inventory `gpu_preference`, else `auto`
(`LLM_ACCEL_AUTO_ORDER`). `auto` skips CUDA/Vulkan/Metal when
`host_accel_available` is false (`CUDA_PATH` is not a host probe). An explicit
`gpu_preference` of `cuda` still spawns a complete CUDA payload if the probe
fails. Incomplete llama-linked Cargo `target/debug/aifs-worker-llm` is not
spawned as a stub; a complete CUDA snapshot is still used if the NVIDIA probe
is a false negative. Discovery then names why CUDA was not used (no
`llm-runtime/cuda`, missing `ggml-cuda`, or NVIDIA probe failed at
`%SystemRoot%\System32\nvcuda.dll`).
`AIFS_WORKER_LLM` still overrides discovery; library search is that file's
directory.

Spawn prepends **only** the payload directory to `PATH` /
`LD_LIBRARY_PATH` / `DYLD_LIBRARY_PATH`. Hello/spawn failures that look like a
missing native library name the payload directory and missing required prefixes
(`llama`, `ggml`, `ggml-cuda`, `ggml-vulkan`) without repeating the NTSTATUS
essay. `get_models` lists complete payloads as `llm_payloads` without spawning
hello.

`pnpm llama` / `llama:cuda` / `llama:vulkan` each compile **one** accelerator
and stage that payload. `AIFS_LLM_FEATURES=cuda,vulkan` is refused (two
payloads, two cargo builds). A CPU rebuild does not delete an existing
`llm-runtime/cuda` snapshot.

See `crates/aifs-protocol/src/llm_payload.rs` and
`crates/aifs-worker-client/src/payload.rs`.
