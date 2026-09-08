# Contributing

Work from the **repository root**. The Cargo workspace, fixtures, docs, and
desktop app all live here (there is no nested `rust/` directory).

## Prerequisites

- Rust stable via [`rust-toolchain.toml`](rust-toolchain.toml) (edition 2024, `rust-version` 1.98)
- For the desktop shell: Node.js 24 LTS and pnpm 12 (root `package.json`)
- Linux desktop builds need WebKitGTK 4.1 development packages (same set as CI)
- Local llama.cpp (`pnpm desktop` / `pnpm llama`): clang, CMake, and a C++ compiler
  (`CXX=g++` on Linux). On Windows, `pnpm llama` sets `CMAKE_GENERATOR` when cmake-rs
  would pick Visual Studio 2026 and the installed CMake is older than 4.2. Raw
  `cargo engine-llm` still needs CMake 4.2+ or an explicit `CMAKE_GENERATOR`
  (`Ninja` with Ninja on `PATH`, or `Visual Studio 17 2022` if VS 2022 is installed).

## Everyday commands

```bash
pnpm install
pnpm build
pnpm test
pnpm desktop
pnpm cli -- scan fixtures/inbox-mixed
pnpm cli -- organize fixtures/inbox-mixed
pnpm cli -- compare fixtures/inbox-mixed fixtures/inbox-mixed.expected.json
```

`pnpm cli --` is `cargo aifs` / `cargo run -p aifs-cli --` (do not add a second `--` after `cargo aifs`). `pnpm build` compiles `aifs-engine`, `aifs`, and the four workers (LLM **stub**)
into `target/debug/`. `pnpm desktop` then rebuilds `aifs-worker-llm` with llama.cpp.

Local llama.cpp (also used by `pnpm desktop`):

```bash
pnpm llama
# Unix: cargo engine-llm
# Windows: prefer pnpm llama, or set CMAKE_GENERATOR (see README)
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
`apps/desktop/src/*.test.ts` (`pnpm test:desktop` from the repo root). Do not click
Apply on the committed fixtures unless you copied them first.
