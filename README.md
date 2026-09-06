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

See [`rust/docs/architecture.md`](rust/docs/architecture.md),
[`rust/docs/domain-model.md`](rust/docs/domain-model.md),
[`rust/docs/protocol.md`](rust/docs/protocol.md), and the
[`golden-path execution plan`](rust/docs/golden-path-execution-plan.md).

## Quick start

```bash
cd rust
cargo run -p aifs-cli -- scan /path/to/folder
cargo run -p aifs-cli -- organize /path/to/folder          # dry run
cargo run -p aifs-cli -- organize /path/to/folder --apply
cargo run -p aifs-cli -- chat /path/to/folder "Move podcasts away from music"
cargo run -p aifs-cli -- compare /path/to/folder --expected fixtures/inbox-mixed.expected.json
```

The CLI locates `aifs-engine` next to itself, via `$AIFS_ENGINE`, or under
`target/{debug,release}/`. There is no in-process fallback. Scan will spawn
`aifs-worker-media` (and document/vision stubs) when those binaries are on the
same path; media-tag extraction falls back to in-process Rust readers if the
media worker is missing.

```bash
cd rust
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

The toolchain is pinned to `stable` (`rust/rust-toolchain.toml`). `unsafe_code` is
forbidden in the workspace; crash-prone native libraries belong in worker processes.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
