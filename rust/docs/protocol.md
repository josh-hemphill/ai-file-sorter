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

Every request has an `id` chosen by the client. Events echo that `id`; unsolicited events
(e.g. `shutdown` because stdin closed) omit it.

## Events

- `progress { stage, current, total?, message }` — may repeat; never terminal.
- `log { level, message }` — never terminal.
- `ready`, `scan_completed`, `revision`, `planned`, `journal`, `chat_reply`,
  `cancelled`, `failed`, `shutdown` — terminal for their request.

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
`undo`, `chat`, `cancel`, and `shutdown`. `chat` interprets the utterance into
deterministic tools (`search`, `inspect`, `structure`, `group`, `rename`,
`validate`) and, when those tools emit patches, stores a child revision authored
by the mock assistant. The assistant never receives SQL or raw filesystem
operations. `plan` requires accepted placements (the CLI `organize` command
accepts all heuristic placements, then dry-runs by default).
