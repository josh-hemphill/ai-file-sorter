# Desktop workspace

Tauri 2 shell + Vue 3 UI. The WebView never opens user files; it calls typed
commands that forward JSONL to `aifs-engine`.

```bash
cd rust
cargo build -p aifs-engine
cd apps/desktop
pnpm install
pnpm test
pnpm tauri dev
```

If the engine binary is not next to the desktop executable, set `AIFS_ENGINE`.
