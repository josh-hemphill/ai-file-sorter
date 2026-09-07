# Architecture

## Processes

```text
apps/desktop (Tauri 2 + Vue 3)
    │  typed commands / events over JSONL
    ▼
bins/aifs-engine
    ├── owns the SQLite workspace store
    ├── scans roots, detects relationships, extracts evidence
    ├── builds and patches proposal revisions
    ├── validates revisions into operation plans
    ├── applies plans through a journal (recovery + undo)
    └── supervises workers
          ├── `aifs-worker-media` (Rust tag readers; later ffprobe)
          ├── `aifs-worker-document` (PDF/Office/text)
          ├── `aifs-worker-vision` (EXIF; describe is LLM)
          └── `aifs-worker-llm` (session-lived; stub by default, llama.cpp with `--features llama`)
```

Rules:

- The UI process never opens user files, SQLite, or model runtimes. It only talks to the
  engine over the protocol in `crates/aifs-protocol`.
- The Tauri shell launches a fixed engine binary with scoped capabilities. Release builds
  fail loudly if the engine is missing; there is no in-process fallback.
- Only the engine writes the store. Workers return evidence/artefacts and never mutate
  files or the database.
- Native, crash-prone libraries live in disposable worker processes so a bad file cannot
  take down the engine or the UI.
- Process isolation is a robustness boundary, not a security sandbox. Path validation,
  protocol validation, timeouts, and resource limits still apply.

## Crates

| Crate | Responsibility | Depends on |
|-------|----------------|------------|
| `aifs-domain` | Plain-data model: assets, bundles, evidence, revisions, plans, journals | serde |
| `aifs-protocol` | JSONL request/event types and request options | domain |
| `aifs-scanner` | Walk a root, assign asset ids, identity, project protection | domain, protocol, relationships |
| `aifs-relationships` | Sidecar / series / archive-part / project bundle detectors | domain |
| `aifs-extractors` | Media tags, document text, and image EXIF → evidence | domain |
| `aifs-store` | SQLite WAL store for snapshots, revisions, plans, journals | domain |
| `aifs-planner` | Heuristic proposals and plan validation | domain, protocol |
| `aifs-apply` | Journaled local apply + undo | domain, scanner |
| `aifs-ai-tools` | Keyword tools that emit `RevisionPatch`es (never SQL or FS ops) | domain, planner |
| `aifs-engine` | Request dispatch (`hello` … `chat`); supervises workers and analysis | domain, protocol, store, workers |
| `aifs-worker-runtime` / `aifs-worker-client` | Worker JSONL loop and spawn/timeout client | protocol |
| `bins/aifs-engine` | Stdio JSONL server | `aifs-engine` |
| `bins/aifs-worker-*` | Isolated extractors (evidence only; never SQLite or FS mutation) | runtime, extractors |
| `bins/aifs-cli` (`aifs`) | `aifs scan` / `organize` / `chat` via the engine process | engine-client |
| `apps/desktop` | Tauri 2 shell + Vue 3 workspace; JSONL to the engine | engine-client, protocol |

## Data flow

1. `scan` → `WorkspaceSnapshot` (entries, skipped, projects, bundles, relationships,
   evidence). Persisted; never mutated.
2. `propose` → root `ProposalRevision` from heuristics.
3. `patch` (user) or `chat` (assistant tools) → child revision. The chain is the
   review history. Chat never mutates disk; it only patches the proposal.
4. `plan` → `OperationPlan` or a list of `PlanIssue`s. Hard-bundle splits, protected
   members, collisions, and path escapes are errors.
5. `apply` → `ApplyJournal`, written before each operation. Dry run produces a journal
   with every entry `intended`.
6. `undo` → reverses a completed journal in reverse order with identity checks.

Fixture regression (`fixtures/inbox-mixed`) compares heuristic destinations,
protected projects, and sidecar stems against a committed JSON map. The Qt
engine is not in this fork, so “old vs new” is rust-engine vs golden plans.

## Filesystem safety

Filesystem and SQLite cannot be one transaction, so apply is a journaled saga:

1. Freeze the plan and re-check every source identity.
2. Journal each operation as `intended` before running it.
3. Same-volume moves use atomic rename; cross-volume falls back to copy → verify → delete.
4. Failures stop the run; the journal records what is done so the user can undo or retry.
5. Applying one member of a hard bundle is rejected unless the user breaks the bundle.

## UI shape (target)

```text
┌ Sources / Sessions ┬ Structure · Items · Relationships · Activity ┬ Assistant / Inspector ┐
│ recent roots       │ proposed tree, dense table, bundle graph,     │ chat, evidence,       │
│ presets            │ warnings & recovery                            │ revision diff         │
├────────────────────┴───────────────────────────────────────────────┴───────────────────────┤
│ background tasks · validation summary · Preview · Apply                                    │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

Intent presets (Tidy inbox, Build archive, Media library, Custom) set scan and proposal
options; advanced analysis controls belong in a workspace settings drawer. Setup (model
download, local/remote endpoints per analysis slot) is a separate first-run surface.

The implementation sequence for that workspace — live scan/analysis stream, mixed-tree
folder roles, review-by-unit, preview diff, settings, and setup — is
[`golden-path-execution-plan.md`](golden-path-execution-plan.md).
