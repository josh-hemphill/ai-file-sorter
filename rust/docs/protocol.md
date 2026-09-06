# Engine protocol

`crates/aifs-protocol` defines the wire format between the UI shell (or CLI/tests) and
`aifs-engine`. Transport is stdio: the client writes one JSON `Request` per line to the
engine's stdin; the engine writes one JSON `Envelope` per line to stdout. Stderr is
diagnostic only.

## Versioning

- `PROTOCOL_VERSION` is bumped on breaking changes. `hello` carries the client's version;
  a mismatch yields `failed { code: incompatible_protocol }`.
- Additive changes keep old fields working. Options structs use `#[serde(default)]` so
  older clients can omit new fields.

## Requests

| `type` | Payload | Terminal event |
|--------|---------|----------------|
| `hello` | `client`, `protocol_version` | `ready` |
| `scan` | `root`, `options: ScanOptions`, optional `session` | `scan_completed { snapshot }` |
| `propose` | `session`, `policy: ProposalPolicy` | `revision` |
| `patch` | `session`, `base_revision`, `author`, `summary`, `patches[]` | `revision` |
| `plan` | `session`, `revision` | `planned { plan, issues }` or `failed { plan_rejected, issues }` |
| `apply` | `session`, `plan`, `dry_run` | `journal` |
| `undo` | `session`, `journal` | `journal` |
| `chat` | `session`, `revision`, `utterance` | `chat_reply { message, revision? }` |
| `cancel` | `target` | `cancelled` on the target request |
| `shutdown` | — | `shutdown` |
| `get_settings` | — | `settings { settings }` |
| `put_settings` | `settings: AppSettings` | `settings { settings }` or `failed { invalid_request }` |
| `get_models` | — | `models { inventory }` (API keys omitted; `artifacts` scanned from disk) |
| `put_models` | `inventory: ModelInventory` | `models { inventory }` |
| `download_model` | `catalog_id` | `models { inventory }` (progress `stage=download`; existing files skipped) |
| `probe_endpoint` | flattened `ModelBackend`, optional `api_key` | `endpoint_probed { ok, message }` |

Every request has an `id` chosen by the client. Events echo that `id`; unsolicited events
(e.g. `shutdown` because stdin closed) omit it.

## Events

- `progress { stage, current, total?, message }` — may repeat; never terminal.
- `log { level, message }` — never terminal. Scan uses this for projects, skips, and
  bundles so the UI can show a live identification stream.
- `ready`, `scan_completed`, `revision`, `planned`, `journal`, `chat_reply`,
  `settings`, `models`, `endpoint_probed`, `cancelled`, `failed`, `shutdown` — terminal for their request.

`Envelope::is_terminal()` encodes this so clients can await completion generically.

## Error codes

`incompatible_protocol`, `invalid_request`, `not_found`, `invalid_root`,
`plan_rejected`, `io`, `storage`, `cancelled`, `internal`.

## Example

```json
{"id":"1","type":"hello","client":"aifs-desktop","protocol_version":1}
{"id":"1","type":"ready","engine_version":"0.1.0","protocol_version":1,"capabilities":[]}
{"id":"2","type":"scan","root":"/home/me/Downloads","options":{"recursive":false}}
{"id":"2","type":"progress","stage":"scan","current":120,"total":null,"message":"Downloads"}
{"id":"2","type":"scan_completed","snapshot":{...}}
```

This slice implements `hello`, `scan`, `propose`, `patch`, `plan`, `apply`,
`undo`, `chat`, `cancel`, `get_settings`, `put_settings`, `get_models`,
`put_models`, `download_model`, `probe_endpoint`, and `shutdown`. `chat` interprets the utterance into
deterministic tools (`search`, `inspect`, `structure`, `group`, `rename`,
`validate`) and, when those tools emit patches, stores a child revision authored
by the mock assistant. The assistant never receives SQL or raw filesystem
operations. `plan` requires accepted placements (the CLI `organize` command
accepts all heuristic placements, then dry-runs by default).

## Workers

The engine speaks a second JSONL dialect with disposable worker processes
(`crates/aifs-protocol::worker`). Transport is the same (one JSON object per
line). Workers never open SQLite and never mutate user files.

| `type` | Payload | Terminal event |
|--------|---------|----------------|
| `hello` | `worker`, `protocol_version` | `ready` |
| `extract` | `root`, `entry` | `extracted { evidence? }` |
| `load` | `backend`, `gpu_preference`, optional `n_gpu_layers`, optional `api_key`, `storage_dir` | `loaded { device, model, n_gpu_layers, fallback? }` |
| `unload` | — | `unloaded` |
| `categorize` | `root`, `entry`, `evidence[]` | `inferred { evidence? }` |
| `describe` | `root`, `entry`, `evidence[]` | `inferred { evidence? }` |
| `chat` | `utterance`, `context` | `chat_completed { message }` |
| `shutdown` | — | `shutdown` |

Binaries: `aifs-worker-media`, `aifs-worker-document`, `aifs-worker-vision`,
`aifs-worker-llm`. Discovery uses `$AIFS_WORKER_MEDIA` (and siblings), then a
binary next to the engine, then `target/{debug,release}/`. Scan prefers a live
media worker and falls back to in-process Rust tag readers when that binary is
missing. Document and vision workers are stubs that return low-confidence
detector evidence. The LLM worker accepts `load` / `unload` / `categorize` /
`describe` / `chat` and currently returns stub `local_model` evidence; llama.cpp
is not linked yet. `api_key` on `load` is never written to logs.

