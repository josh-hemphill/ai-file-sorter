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
| `scan` | `root`, `options: ScanOptions`, optional `session` | `scan_completed { snapshot }` or `cancelled` |
| `propose` | `session`, `policy: ProposalPolicy` | `revision` |
| `patch` | `session`, `base_revision`, `author`, `summary`, `patches[]` | `revision` |
| `plan` | `session`, `revision` | `planned { plan, issues }` or `failed { plan_rejected, issues }` |
| `apply` | `session`, `plan`, `dry_run` | `journal` |
| `undo` | `session`, `journal` | `journal` |
| `chat` | `session`, `revision`, `utterance` | `chat_reply { message, revision? }` |
| `cancel` | `target` | ack `cancelled` on the cancel request; the **target** also ends with `cancelled` (scan) |
| `shutdown` | — | `shutdown` |
| `get_settings` | — | `settings { settings }` |
| `put_settings` | `settings: AppSettings` | `settings { settings }` or `failed { invalid_request }` |
| `get_models` | — | `models { inventory }` (API keys omitted; `artifacts` scanned from disk; each slot includes `runtime`: `off`, `stub`, `hosted`, `llama`, `missing_worker`, or `missing_files`) |
| `put_models` | `inventory: ModelInventory` | `models { inventory }` (same redaction and `runtime` as `get_models`; `runtime` is not stored) |
| `download_model` | `catalog_id` | `models { inventory }` (progress `stage=download`; SHA-256 verified; matching files skipped; mismatch deletes the junk file; `runtime` recomputed) |
| `probe_endpoint` | flattened `ModelBackend`, optional `api_key` | `endpoint_probed { ok, message }` |

Every request has an `id` chosen by the client. Events echo that `id`; unsolicited events
(e.g. `shutdown` because stdin closed) omit it.

## Events

- `progress { stage, current, total?, message }` — may repeat; never terminal.
  Scan/analyze stages include `scan`, `relationships`, `extract`, `categorize`,
  and `describe`.
- `log { level, message }` — never terminal. Scan uses this for projects, skips, and
  bundles so the UI can show a live identification stream.
- `ready`, `scan_completed`, `revision`, `planned`, `journal`, `chat_reply`,
  `settings`, `models`, `endpoint_probed`, `cancelled`, `failed`, `shutdown` — terminal for their request.

`Envelope::is_terminal()` encodes this so clients can await completion generically.

`cancel` is decoded on a stdin reader thread so a long `scan` can notice the flag
between files. A cancelled scan emits `cancelled` for the scan id and does **not**
emit `scan_completed`. If walk/relationships finished, the engine still writes a
**checkpoint** snapshot for that `session` (extract/analyze evidence flushed every
8 new bags; skips do not count). The next `scan` with the same `session` and root carries matching
identities forward and skips files that already have metadata. Apply/undo that
already mutated disk still emit `journal` (partial/failed) rather than pretending
the work never happened.

Pass `session` on `scan` to resume. The desktop shell keeps a per-root session id
so Cancel then Scan continues extract instead of starting over.

The engine-client idle wait is 180s per event (reset on every progress/log line)
so categorize/describe can exceed 60s as long as workers keep emitting.

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
`put_models`, `download_model`, `probe_endpoint`, and `shutdown`. `chat` prefers
model `RevisionPatch` JSON when the chat slot is assigned. Keyword tools
(`search`, `inspect`, `structure`, `group`, `rename`, `validate`) run when the
slot is off, the worker fails, or the reply is not parseable JSON with a
`patches` key. Child revisions
are authored by the assistant model when patches apply. The assistant never
receives SQL or raw filesystem operations. `plan` requires accepted placements
(the CLI `organize` command accepts all heuristic placements, then dry-runs by
default).

## Workers

The engine speaks a second JSONL dialect with disposable worker processes
(`crates/aifs-protocol::worker`). Transport is the same (one JSON object per
line). Workers never open SQLite and never mutate user files.

| `type` | Payload | Terminal event |
|--------|---------|----------------|
| `hello` | `worker`, `protocol_version` | `ready` |
| `extract` | `root`, `entry` | `extracted { evidence? }` |
| `load` | `backend`, `gpu_preference`, optional `n_gpu_layers`, optional `api_key`, `storage_dir` | `loaded { device, model, n_gpu_layers, fallback? }` (llama.cpp retries **once** on CPU after GPU init/OOM; `fallback` names the reason. Context-window errors are not retried. `AIFS_N_GPU_LAYERS` applies when `n_gpu_layers` is omitted.) |
| `unload` | — | `unloaded` |
| `categorize` | `root`, `entry`, `evidence[]` | `inferred { evidence? }` |
| `describe` | `root`, `entry`, `evidence[]` | `inferred { evidence? }` |
| `chat` | `utterance`, `context` | `chat_completed { message }` |
| `shutdown` | — | `shutdown` |

Binaries: `aifs-worker-media`, `aifs-worker-document`, `aifs-worker-vision`,
`aifs-worker-llm`. Discovery uses `$AIFS_WORKER_MEDIA` (and siblings), then a
binary next to the engine, then `target/{debug,release}/`. Scan prefers a live
media worker and falls back to in-process Rust tag readers when that binary is
missing. The document worker extracts PDF/Office/text (with an in-process
fallback). The vision worker reads EXIF (`image.captured_on`, `image.camera`,
and GPS when present) with an in-process fallback; image *description* is an
LLM `describe` later. After extract, scan loads the LLM worker once per distinct
slot backend and runs `categorize` / `describe` when those slots are not off.
Model `category` is a whitelist hint for propose, not a trusted path. The LLM
worker accepts `load` / `unload` / `categorize` / `describe` / `chat` and
currently returns stub `local_model` evidence unless the worker is built with
llama.cpp (`pnpm desktop` / `pnpm llama` / `cargo engine-llm`, optional `cuda` / `vulkan` / `metal`)
or a hosted HTTP backend is loaded (`RemoteModel` evidence). GPU init or OOM on
`load` retries **once** with `n_gpu_layers=0`; scan logs `fallback` (no modal).
The string `prompt exceeds the llama.cpp context window` is not a GPU failure.
`AIFS_N_GPU_LAYERS` sets offload when `n_gpu_layers` is omitted. Local describe
loads mmproj via libmtmd: JPEG/PNG/WebP at most 8 MiB attach pixels; RAW, oversize,
and other image types use filename + EXIF only (scan logs RAW). Hosted describe
never uploads pixels. Scan runs **describe before categorize** so captions are
prior evidence on the categorize turn. `categorize` accepts additive
`allowed_categories` (whitelist) and `style` fields; omitted fields default to
unconstrained / consistent. Screenshot/UI captures log
`{path} · screenshot · Screenshots/UI`. Default
`cargo test --workspace` does not compile llama.cpp. Ubuntu CI has a separate
`llama-cpu` job that compiles the worker with `--features llama` and does not
download Gemma. `get_models` fills each slot's
`runtime` (`off`, `stub`, `hosted`, `llama`, `missing_worker`, `missing_files`)
from worker hello and files on disk; scan logs the same kinds instead of claiming
images will be described when infer is stubbed. Hosted probes contact the
endpoint. Chat asks the model for `RevisionPatch` JSON and applies those
patches; keyword `interpret()` is the fallback when the chat slot is off, the
worker fails, or the reply is not parseable JSON with a `patches` key (a
parseable empty `patches` array means the model is only answering).
`api_key` on `load` is never written to logs.

