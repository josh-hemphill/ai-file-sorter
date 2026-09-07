# Contributing

Work from the **repository root**. The Cargo workspace, fixtures, docs, and
desktop app all live here (there is no nested `rust/` directory).

## Prerequisites

- Rust stable via [`rust-toolchain.toml`](rust-toolchain.toml) (edition 2024, `rust-version` 1.98)
- For the desktop shell: Node.js 24 LTS and pnpm 12 (`apps/desktop/package.json`)
- Linux desktop builds need WebKitGTK 4.1 development packages (same set as CI)
- Local llama.cpp (`make desktop` / `cargo engine-llm`): clang, CMake, and a C++ compiler
  (`CXX=g++` on Linux)

## Everyday commands

```bash
make build
make test
make desktop
cargo aifs scan fixtures/inbox-mixed
cargo aifs organize fixtures/inbox-mixed
cargo aifs compare fixtures/inbox-mixed fixtures/inbox-mixed.expected.json
```

`cargo aifs` is an alias for `cargo run -p aifs-cli --` (do not add a second `--`). `make build` compiles `aifs-engine`, `aifs`, and the four workers (LLM **stub**)
into `target/debug/`. `make desktop` then rebuilds `aifs-worker-llm` with llama.cpp.

Local llama.cpp (also used by `make desktop`):

```bash
make llama
# or: cargo engine-llm
```

## Layout

| Path | What it is |
|------|------------|
| `crates/` | Library crates (domain, protocol, engine, planner, …) |
| `bins/` | `aifs`, `aifs-engine`, `aifs-worker-*` |
| `apps/desktop/` | Tauri 2 + Vue 3 shell |
| `fixtures/` | Golden mixed-tree folders for `compare` / engine tests |
| `docs/` | Architecture, protocol, domain model, golden-path plan |

Open this root in rust-analyzer. Do not point it at a nested crate unless you
are debugging that crate in isolation.

## Tests

`cargo test --workspace` is the engine gate. Desktop helper tests live in
`apps/desktop/src/*.test.ts` (`pnpm test` in that directory). Do not click
Apply on the committed fixtures unless you copied them first.
