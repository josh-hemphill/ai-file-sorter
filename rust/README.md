# AI File Sorter — Rust rewrite

This directory holds the Rust rewrite of AI File Sorter. The Qt/C++ application under
`app/` remains the production build while this tree grows to parity. The goals carried
over from the earlier Avalonia exploration are:

- **Process isolation** — the UI never loads inference runtimes, PDF/media decoders, or
  SQLite. A separate `aifs-engine` process owns the workspace and supervises workers.
- **Relationship-aware organisation** — files that belong together (RAW+JPEG, video +
  subtitles, project trees, split archives) are modelled as bundles with constraints, so
  the planner cannot split them by accident.
- **Review-first, journaled mutation** — every organisation proposal is an immutable
  revision; nothing touches disk until a validated plan is applied through a journal that
  supports recovery and undo.
- **Collaborative refinement** — the user and an assistant iterate on the same revision
  chain using a small, auditable patch vocabulary instead of free-form file operations.
- **One window** — a single workspace (Structure / Items / Relationships / Activity with an
  assistant + inspector rail) replaces the current dialog cascade.

See [`docs/architecture.md`](docs/architecture.md), [`docs/domain-model.md`](docs/domain-model.md),
and [`docs/protocol.md`](docs/protocol.md).

## Layout

```text
rust/
  Cargo.toml              workspace
  crates/
    aifs-domain/          plain-data domain model (no I/O)
    aifs-protocol/        JSONL protocol + request options
    aifs-scanner/         root walker, identity, project skip
    aifs-relationships/   sidecar / series / archive / project bundles
    aifs-extractors/      bounded media-tag readers
    aifs-store/            SQLite workspace store
    aifs-planner/          heuristic propose + plan validation
    aifs-apply/            journaled apply + undo
    aifs-engine/           session store + command dispatch
    aifs-engine-client/   spawn + JSONL client
  bins/
    aifs-engine/          stdio JSONL server
    aifs-cli/             `aifs scan <folder>`
  docs/                   design notes
```

Later slices add `aifs-store`, `aifs-planner`, `aifs-apply`, remaining engine
commands, and `apps/desktop` (Tauri 2 + Vue 3).

```bash
cd rust
cargo run -p aifs-cli -- organize /path/to/folder
cargo run -p aifs-cli -- organize /path/to/folder --apply
```

The CLI locates `aifs-engine` next to itself, via `$AIFS_ENGINE`, or under
`target/{debug,release}/`. There is no in-process fallback.

## Working on it

```bash
cd rust
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

The toolchain is pinned to `stable` via `rust-toolchain.toml`; the minimum supported
version is declared in `Cargo.toml` (`rust-version`). `unsafe_code` is forbidden
workspace-wide; anything that needs FFI lives in a dedicated worker binary.

## Conventions

- Crates are named `aifs-<area>`; directories are lowercase with dashes.
- Domain types are immutable values with `serde` derives; behaviour lives in functions.
- Every public item has a one-sentence doc comment (`missing_docs` is a warning).
- `unwrap`/`expect` are lint warnings outside tests; return errors instead.
- Tests live next to the code they cover; fixtures are generated in temp directories.
