# AI File Sorter (Rust rewrite)

Fork of [AI File Sorter](https://github.com/hyperfield/ai-file-sorter) rewritten in
Rust. The production C++/Qt application is **not** in this tree; this repository is
the engine, CLI, and Tauri 2 + Vue 3 workspace.

- **Process isolation** — the UI never loads inference runtimes, PDF/media decoders, or
  SQLite. `aifs-engine` owns the workspace and supervises workers.
- **Relationship-aware organisation** — RAW+JPEG, video+subtitles, project trees, and
  split archives are bundles with hard constraints.
- **Review-first, journaled mutation** — proposals are immutable revisions; nothing
  touches disk until a validated plan is applied through a journal that supports undo.
- **Collaborative refinement** — the user and an assistant share a small patch
  vocabulary. The assistant never gets SQL or raw filesystem operations.

See [`docs/architecture.md`](docs/architecture.md),
[`docs/domain-model.md`](docs/domain-model.md),
[`docs/protocol.md`](docs/protocol.md), and the
[`golden-path execution plan`](docs/golden-path-execution-plan.md).

## From source

Requirements: a Rust stable toolchain (this repo pins it in
[`rust-toolchain.toml`](rust-toolchain.toml)). Desktop also needs Node.js 24 LTS
and [pnpm](https://pnpm.io/) 12.

```bash
git clone https://github.com/josh-hemphill/ai-file-sorter.git
cd ai-file-sorter
pnpm build
pnpm cli -- scan fixtures/inbox-mixed
pnpm cli -- organize fixtures/inbox-mixed          # dry run
pnpm cli -- organize /path/to/folder --apply
pnpm cli -- chat fixtures/inbox-mixed "Move podcasts away from music"
pnpm cli -- compare fixtures/inbox-mixed fixtures/inbox-mixed.expected.json
```

`pnpm build` (or `make build` / `cargo engine-bins`) compiles `aifs-engine`, the `aifs` CLI, and the media / document /
vision / LLM workers into `target/debug/`. The CLI locates `aifs-engine` next to
itself, via `$AIFS_ENGINE`, or under `target/{debug,release}/`. There is no
in-process fallback. Scan prefers isolated workers when those binaries are on
the same path; extractors fall back to in-process Rust readers if a worker is
missing.

Desktop (builds the engine first, then starts Tauri + Vite):

```bash
pnpm install
pnpm desktop
```

Equivalent without the root scripts:

```bash
cargo engine-bins
pnpm llama
pnpm --filter desktop test
pnpm --filter desktop tauri dev
```

If the engine binary is not next to the desktop executable, set `AIFS_ENGINE`.
Packaged desktop builds embed the engine and workers as Tauri sidecars.

```bash
pnpm check          # fmt --check, clippy, cargo test
pnpm test:desktop   # Vue unit tests + vue-tsc
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

`pnpm desktop` / `pnpm llama` compile llama.cpp into `aifs-worker-llm`
(needs clang, CMake, and a C++ compiler). `pnpm llama` and Tauri's
`beforeDevCommand` wrap `cargo engine-llm` so Windows can compile when Visual
Studio 2026 is installed but CMake is older than 4.2 (cmake-rs passes
`-G "Visual Studio 18 2026"`, which those CMake builds do not know). Upgrade
CMake to 4.2+ or set `CMAKE_GENERATOR` yourself (`Ninja`, or
`Visual Studio 17 2022` when VS 2022 is present) if you invoke `cargo engine-llm`
directly. `pnpm build` and `cargo test --workspace` keep the stub worker so
default CI stays fast. CUDA/Vulkan/Metal stay opt-in:

```bash
pnpm llama
CXX=g++ cargo engine-llm
pnpm llama:cuda
cargo build -p aifs-worker-llm --features llama,vulkan
cargo build -p aifs-worker-llm --features llama,metal   # macOS
```

A CUDA build spends most of its time in `llama-cpp-sys-2`'s CMake step. Cargo's
bar stays on that crate with no further output because cmake-rs hides nvcc
unless `CMAKE_VERBOSE` is set. If `CMAKE_CUDA_ARCHITECTURES` is unset, llama.cpp
compiles Maxwell through Blackwell (often 15–60 minutes, or longer if all-core
nvcc starts swapping). That is not a Cargo deadlock.

`pnpm llama:cuda` (and `make llama-cuda`) detect the GPU SM via `nvidia-smi`,
pin `CMAKE_CUDA_ARCHITECTURES`, cap `CMAKE_BUILD_PARALLEL_LEVEL` (4 on Unix, 2
on Windows), and set `CXX=g++` on Linux when those are unset. Equivalent by
hand:

```bash
CMAKE_CUDA_ARCHITECTURES=86 CMAKE_BUILD_PARALLEL_LEVEL=4 CXX=g++ \
  cargo build -p aifs-worker-llm --features llama,cuda
```

SM examples: `75` Turing, `86` RTX 30, `89` RTX 40, `90` Hopper, `120a` Blackwell.
`ps` / Task Manager should show `nvcc` / `cmake` while it runs. `CMAKE_VERBOSE=1`
or `cargo build -vv` prints the CMake log. If a multi-arch compile already
started, stop it and `cargo clean -p llama-cpp-sys-2` before rebuilding with a
pin (`llama-cpp-sys-2` does not rebuild when only `CMAKE_*` env vars change).

Bare `cargo build -p aifs-worker-llm --features llama,cuda` does **not** apply
those caps. `llama-cpp-sys-2` then passes cmake `--parallel` equal to every
CPU (for example 24). On Windows that often fails at link with
`fatal error LNK1136: invalid or corrupt file` on a `ggml-cuda` `.obj` (nvcc
writes a truncated object; MSBuild still tries to pack it into
`ggml-cuda.lib`). Network/mapped drives (`E:\Share\…`, UNC paths) make this
much more likely. Recover in PowerShell:

```powershell
cargo clean -p llama-cpp-sys-2
$env:CMAKE_CUDA_ARCHITECTURES = "86"   # your GPU SM; nvidia-smi --query-gpu=compute_cap
$env:CMAKE_BUILD_PARALLEL_LEVEL = "2"
# If the repo is on a share, keep object files on local NTFS:
$env:CARGO_TARGET_DIR = "C:\aifs-target"
pnpm llama:cuda
```

Linux desktop packages (Debian/Ubuntu), matching CI:

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
  libsoup-3.0-dev libjavascriptcoregtk-4.1-dev librsvg2-dev
```

The toolchain is pinned to `stable` (edition 2024). `unsafe_code` is forbidden in
the workspace; crash-prone native libraries belong in worker processes.

More contributor detail: [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
