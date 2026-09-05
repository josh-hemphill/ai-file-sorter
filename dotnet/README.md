# Avalonia Native AOT conversion

This is the first slice of a parallel rewrite of AI File Sorter as a .NET 9
Avalonia Native AOT application. The Qt/C++ app remains the production UI.

## Why this rewrite exists

The current Qt process does too much in one address space:

- Analysis runs in a worker thread, but llama.cpp, SQLite, and file IO still
  live in the same process as the GUI.
- `AnalysisRuntimeLock` is a global exclusive lock shared by GUI, Explorer, and
  headless jobs. When another owner holds it, the Analyze button is disabled and
  the UI looks frozen.
- SQLite is opened without WAL or a busy timeout, so a second process can block
  on the categorization cache.
- File reads can wait on exclusive locks held by Office, OneDrive, or media
  players.

The Avalonia architecture splits those concerns:

1. **UI process** (`aifs-ui`): Avalonia with compiled bindings. It never opens
   user files or SQLite on the UI thread.
2. **Engine** (`aifs engine` or an in-process worker): scan, categorize, persist.
   Files are opened with `FileShare.ReadWrite | FileShare.Delete`. Locked files
   are skipped after a short timeout instead of hanging.
3. **SQLite suggestion database** via `SQLitePCLRaw` (WAL + busy timeout), not
   `Microsoft.Data.Sqlite`. The UI table is a compiled-binding grid, not
   Avalonia DataGrid.
4. **Local GGUF is a sidecar** (`aifs-llama`). Embedding llama.cpp in the UI
   process would recreate the freeze. The AOT UI only selects and downloads
   GGUF files; inference stays out of `aifs-ui`.

## What this slice ports and adds

Ported from the Qt app:

- File-family classification (image, document, audio, video, archive, …)
- Protected project detection rules (Unity, Unreal, Godot, Git, Node, Python,
  Rust, Go, Gradle, .NET, Xcode, Blender)
- ID3 / FLAC / OGG / MP4 metadata reads for rename suggestions

New in this slice:

- Audio/video **content** categorization from tags, duration, and filename cues
  (Podcasts, Audiobooks, Music, Screen Recordings, Camera Footage, TV, …)
- Project folders are not only skipped; they become **zip/tar archive-entity
  suggestions** so a Unity/Git tree can be filed as one item
- Versioned `aifs.filingPlan.v1` JSON plus a compact remote-handoff prompt so a
  stronger model can propose **per-file relative paths**
- Local SQLite suggestion database (`suggestions.sqlite`) with WAL; remote
  proposals merge as `RemoteProposed` rows and never move files
- Qt-parity content controls (subcategories, files/folders, document/image
  options, audio/video metadata rename) and LLM selection (OpenAI, Gemini,
  custom API, local GGUF catalog, visual backends)
- Isolated `aifs engine` stdio protocol for crash and lock isolation
- Headered suggestions table with editable proposed paths, accept/keep/reject,
  dry-run, and local apply + undo plan

## Layout

```text
dotnet/
  src/AiFileSorter.Core     AOT-safe domain library
  src/AiFileSorter.Cli      Headless CLI + engine worker (`aifs`)
  src/AiFileSorter.App      Avalonia Native AOT UI (`aifs-ui`)
  src/AiFileSorter.Llama    CPU GGUF sidecar (`aifs-llama`, not Native AOT)
  tests/AiFileSorter.Core.Tests
```

## Commands

```bash
export PATH="$HOME/.dotnet:$PATH"   # if the SDK is user-installed
dotnet test dotnet/AiFileSorter.sln
dotnet run --project dotnet/src/AiFileSorter.Cli -- analyze /path/to/folder --recursive
dotnet run --project dotnet/src/AiFileSorter.Cli -- handoff /path/to/folder --prompt-only
dotnet run --project dotnet/src/AiFileSorter.Cli -- merge proposal.json --path /path/to/folder
dotnet run --project dotnet/src/AiFileSorter.Cli -- apply /path/to/folder --dry-run
dotnet publish dotnet/src/AiFileSorter.Cli/AiFileSorter.Cli.csproj -c Release -r linux-x64
dotnet run --project dotnet/src/AiFileSorter.Cli -- models
dotnet run --project dotnet/src/AiFileSorter.Cli -- download gemma-3-4b-it
dotnet publish dotnet/src/AiFileSorter.Llama/AiFileSorter.Llama.csproj -c Release -r linux-x64 --self-contained
```

Native AOT publish needs the platform C toolchain (`clang` and `zlib` on Linux,
MSVC on Windows, Xcode on macOS). AOT does not cross-compile across OS families.

## Local GGUF models

Select LLM downloads the same suggested files as the Qt dialog:

- **Categorization & documents:** Gemma 3 4B IT, Mistral 7B Instruct, Gemma 1.1 7B IT, legacy LLaMa 3B
- **Image analysis:** Gemma 3 4B IT (text + mmproj) and LLaVA 1.6 Mistral 7B (text + mmproj)

Downloads are resumable (`.part` + `.aifs.meta`), validate the `GGUF` magic header, and land in the Qt storage directory (`~/.local/share/aifilesorter/llms` on Linux) so the two UIs can share files. URLs come from `app/resources/.env` and can be overridden with the same environment variables (`LOCAL_LLM_3B_DOWNLOAD_URL`, `GEMMA3_4B_MMPROJ_URL`, …).

Inference is **not** compiled into `aifs-ui`. Publish `aifs-llama` (LLamaSharp CPU, not Native AOT) beside the UI. The engine calls that sidecar for local categorization, document excerpts, and JSON path proposals.

## AOT-safe SQLite and tables

`Microsoft.Data.Sqlite` is not used: `SqliteConnectionStringBuilder` trips IL2113
in Native AOT. The engine talks to SQLite through `SQLitePCLRaw.bundle_e_sqlite3`
and explicit `sqlite3_*` calls (`Batteries_V2.Init()`, WAL, `busy_timeout=5000`).

Avalonia `DataGrid` 11.3 and `TreeDataGrid` are not AOT-clean on this stack
(DataGrid trim/ILC failures; TreeDataGrid AOT is aimed at Avalonia 12). The UI
uses a compiled-binding headered grid instead. JSON remains a fallback when the
database path ends in `.json`.

Libraries considered and skipped for AOT:

- **Microsoft.Data.Sqlite / sqlite-net-pcl / EF Core SQLite**: reflection and
  trim warnings
- **LiteDB / Realm**: not Native AOT friendly
- **DuckDB.NET**: heavier than a suggestion catalog needs
- **Avalonia.Controls.DataGrid**: failed ILC in 11.3
- **Avalonia TreeDataGrid**: wait for Avalonia 12 / Accelerate if a virtualized
  tree is required later

## Remote proposal loop

1. Analyze locally and save rows to SQLite (`localRelativePath`, status `Local`).
2. Export a compact handoff or ask a configured remote model. The model returns
   `updates[].proposedRelativePath`.
3. Merge into the database as `RemoteProposed`. Disk is unchanged.
4. Review the table: accept remote, keep local, or edit the proposed path.
5. Dry-run, then **Apply locally**. An `aifs-undo-plan.json` is written next to
   the folder.

## Phased path to feature parity

1. **This PR**: UI isolation, lock-tolerant IO, media content categories,
   archive-entity suggestions, content-control parity, GGUF select/download,
   SQLite suggestion DB, remote path-proposal merge, local apply, `aifs-llama`
   sidecar for categorization and document excerpts.
2. Real mmproj/LLaVA image inference in the sidecar (downloads already land).
3. Document PDF text extraction (PDFium stays out of the AOT UI).
4. Review/apply/undo polish, then deprecate the Qt UI.

Do not attempt to Native-AOT llama.cpp, PDFium, and the Avalonia UI into one
binary. That would keep the original freeze and lock problems.
