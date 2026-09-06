# Rust workspace

Crates and binaries for the AI File Sorter rewrite. The desktop shell lives in
`apps/desktop` (Tauri 2 + Vue 3) once that slice lands.

See the [repository README](../README.md) and [`docs/`](docs/).

```bash
cargo run -p aifs-cli -- organize /path/to/folder
cargo test --workspace
```
