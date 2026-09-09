# Desktop workspace

Tauri 2 shell + Vue 3 UI. The WebView never opens user files; it calls typed
commands that forward JSONL to `aifs-engine`.

Requires pnpm 12 (pinned in the root `package.json` as `packageManager`) and Node.js 24 LTS.

From the **repository root**:

```bash
pnpm install
pnpm desktop
```

Or:

```bash
cargo engine-bins
pnpm llama
pnpm --filter desktop test
pnpm --filter desktop tauri dev
```

`tauri dev` also builds the engine, workers, and llama.cpp LLM worker via
`beforeDevCommand`. Packaged
builds copy those binaries into `src-tauri/binaries/{stem}-{target-triple}` and
embed them as `externalBin` sidecars. `src-tauri/.taurignore` excludes `binaries/`
so copying sidecars does not restart `tauri dev`. If the engine binary is not next to the
desktop executable, set `AIFS_ENGINE`. Discovery also accepts the Tauri sidecar
filename (`aifs-engine-{target-triple}`).
