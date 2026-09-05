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
3. **JSON plan catalog** for the suggestion database, serialized with a source
   generator so Native AOT does not depend on SQLite reflection.
4. **Local GGUF stays a sidecar later**. Embedding llama.cpp in the UI process
   would recreate the freeze. Phase 3 should keep the existing C++ headless
   binary as an optional worker, not compile it into the Avalonia AOT image.

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
  stronger model can propose a better overall folder structure
- Isolated `aifs engine` stdio protocol for crash and lock isolation

## Layout

```text
dotnet/
  src/AiFileSorter.Core     AOT-safe domain library
  src/AiFileSorter.Cli      Headless CLI + engine worker (`aifs`)
  src/AiFileSorter.App      Avalonia Native AOT UI (`aifs-ui`)
  tests/AiFileSorter.Core.Tests
```

## Commands

```bash
export PATH="$HOME/.dotnet:$PATH"   # if the SDK is user-installed
dotnet test dotnet/AiFileSorter.sln
dotnet run --project dotnet/src/AiFileSorter.Cli -- analyze /path/to/folder --recursive
dotnet run --project dotnet/src/AiFileSorter.Cli -- handoff /path/to/folder --prompt-only
dotnet publish dotnet/src/AiFileSorter.Cli/AiFileSorter.Cli.csproj -c Release -r linux-x64
dotnet publish dotnet/src/AiFileSorter.App/AiFileSorter.App.csproj -c Release -r linux-x64
```

Native AOT publish needs the platform C toolchain (`clang` and `zlib` on Linux,
MSVC on Windows, Xcode on macOS). AOT does not cross-compile across OS families.

## Phased path to feature parity

1. **This PR**: UI isolation, lock-tolerant IO, media content categories,
   archive-entity suggestions, plan handoff.
2. Remote OpenAI/Gemini/custom categorization using the existing credential
   model, implemented in the engine process.
3. Optional local GGUF sidecar via the current C++ `--headless` contract.
4. Document and image content analysis as additional engine stages.
5. Review/apply/undo parity, then deprecate the Qt UI.

Do not attempt to Native-AOT llama.cpp, PDFium, and the Avalonia UI into one
binary. That would keep the original freeze and lock problems.
