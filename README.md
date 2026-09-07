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
make build
cargo aifs scan fixtures/inbox-mixed
cargo aifs organize fixtures/inbox-mixed          # dry run
cargo aifs organize /path/to/folder --apply
cargo aifs chat fixtures/inbox-mixed "Move podcasts away from music"
cargo aifs compare fixtures/inbox-mixed fixtures/inbox-mixed.expected.json
```

`make build` compiles `aifs-engine`, the `aifs` CLI, and the media / document /
vision / LLM workers into `target/debug/`. The CLI locates `aifs-engine` next to
itself, via `$AIFS_ENGINE`, or under `target/{debug,release}/`. There is no
in-process fallback. Scan prefers isolated workers when those binaries are on
the same path; extractors fall back to in-process Rust readers if a worker is
missing.

Desktop (builds the engine first, then starts Tauri + Vite):

```bash
make desktop
```

Equivalent without Make:

```bash
cargo engine-bins
cargo engine-llm
cd apps/desktop
pnpm install
pnpm test
pnpm tauri dev
```

If the engine binary is not next to the desktop executable, set `AIFS_ENGINE`.
Packaged desktop builds embed the engine and workers as Tauri sidecars.

```bash
make check          # fmt --check, clippy, cargo test
make desktop-test   # Vue unit tests + vue-tsc
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

`make desktop` / `cargo engine-llm` compile llama.cpp into `aifs-worker-llm`
(needs clang, CMake, and a C++ compiler). `make build` and `cargo test --workspace`
keep the stub worker so default CI stays fast. CUDA/Vulkan/Metal stay opt-in:

```bash
make llama
CXX=g++ cargo engine-llm
CXX=g++ cargo build -p aifs-worker-llm --features llama,cuda
cargo build -p aifs-worker-llm --features llama,vulkan
cargo build -p aifs-worker-llm --features llama,metal   # macOS
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
