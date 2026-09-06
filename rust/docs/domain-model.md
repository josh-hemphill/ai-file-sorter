# Domain model

All types live in `crates/aifs-domain` and are plain serialisable values.

## Identity

- `AssetId`, `BundleId`, `RevisionId`, `SessionId`, `PlanId`, `JournalId` are opaque UUIDs.
- Paths are **never** identity. `FileIdentity` (device, inode, size, mtime, optional
  content fingerprint) is captured at scan time so apply/undo can detect that a file
  changed underneath.
- `RelativePath` is the only path type that crosses the engine boundary in proposals and
  plans. It is normalised (`/` separators, no `.`/`..` except the session-root sentinel
  `.`), rejects absolute paths and Windows-hostile names, and can be resolved under a
  session root safely. `RelativePath::session_root()` (`.`) means "the scanned folder
  itself", used when a project is detected at the session root.

## Observation

- `ObservedEntry` — one file or directory under the root: id, relative path, kind,
  `FileFamily`, identity, hidden flag, `LockState`.
- `SkippedEntry` — what the scanner excluded and why (`protected_project`, `symlink`,
  `hidden`, `junk`, `depth_limit`, `error`). Surfaced in the UI so users see what was
  ignored.
- `ProjectMatch` — a recognised project root with `Strong` (protected, not traversed) or
  `Weak` (hint only) strength.

## Relationships and bundles

- `Relationship` — a typed edge (`sidecar`, `subtitle`, `cover_art`, `project_member`,
  `series_member`, `archive_part`, `derived`, `duplicate`, `hard_link`) with confidence
  and detector provenance.
- `Bundle` — assets organised as one unit, with a `BundleConstraint`:
  - `soft` — hint only.
  - `move_together` — same destination folder.
  - `preserve_layout { root }` — keep relative layout; the root moves as a unit.
  - `protected { reason }` — do not move.
- Hard constraints come from deterministic detectors. Assistants may suggest soft
  relationships; they cannot override a hard constraint without an explicit user action.

## Evidence

`Evidence` is a bag of string facts about one asset from one `EvidenceSource`
(filesystem, media tags, EXIF, document metadata, detector, local model, remote model,
user). Well-known keys live in `evidence::keys`. Evidence is treated as untrusted input
(prompt-injection surface) and model-produced evidence is kept distinct so it can be
excluded from remote prompts.

## Proposals

- `ProposalRevision` — immutable; one `Placement` per asset; `parent` links form the
  review history. `RevisionAuthor` records who produced it.
- `Placement` — destination path, rationale, `SuggestionOrigin`, `ReviewState`
  (`proposed` / `accepted` / `rejected`).
- `RevisionPatch` — the shared edit vocabulary for the UI and assistants:
  `set_destination`, `move_to_folder`, `rename`, `accept`, `reject`, `reopen`. Applying
  patches yields a child revision and resets touched placements to `proposed`.

## Plans and journals

- `OperationPlan` — ordered `create_directory` / `move` / `remove_empty_directory`
  operations derived from accepted placements, plus warnings.
- `PlanIssue` — validation finding with a stable `code`, severity, and affected assets.
- `ApplyJournal` — per-operation `JournalEntry` states (`intended`, `done`, `skipped`,
  `failed`, `rolled_back`) and an overall `JournalStatus`.
