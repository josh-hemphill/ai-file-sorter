# Rust workspace

Crates and binaries for the AI File Sorter rewrite. The desktop shell lives in `apps/desktop` (Tauri 2 + Vue 3).

See the [repository README](../README.md) and [`docs/`](docs/).

```bash
cargo run -p aifs-cli -- organize /path/to/folder
cargo test --workspace
```

Optional llama.cpp stays out of default CI. If `c++` is Clang without
libstdc++ headers, set `CXX=g++`:

```bash
CXX=g++ cargo build -p aifs-worker-llm --features llama
CXX=g++ cargo build -p aifs-worker-llm --features llama,cuda
```
