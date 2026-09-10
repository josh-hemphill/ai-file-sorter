# Golden path execution plan

This is the implementation plan for turning the Tauri/Vue workspace into a review-first
organiser that can handle messy mixed trees: flat dumps of related files, nested
projects that must stay intact, and leftover archive attempts whose folder names still
carry meaning. It also covers the missing scan/analysis stream, a settings surface
that matches upstream classification controls, and a setup surface for local/remote
models and custom endpoints.

Do not estimate calendar time from this document. Sequence is by dependency and risk:
shell and protocol first, then transparency, then mixed-tree intelligence, then
review/apply, then settings/setup, then real model inference, then scale.

Local llama.cpp / catalog-download follow-ups taken from upstream Qt live in
[`upstream-model-plan.md`](upstream-model-plan.md).

## Success state

A skeptical user can:

1. Point the app at a large, mixed root without fearing immediate mutation.
2. Watch a live, cancellable scan/analysis stream while files are identified.
3. See what was protected, skipped, or treated as a unit *before* reviewing files.
4. Approve or reject groups (projects, sidecars, archives, libraries), not only files.
5. Preview an actual from→to diff, then Apply with confirmation and scoped Undo.
6. Configure classification like upstream (mode, whitelist, language, analysis toggles).
7. Download or point at local/remote models independently for image description,
   document analysis, categorization, and assistant chat.

A skeptical engineer can:

- Replay the mixed-tree fixture plus a nested “junk drawer” fixture through CLI and UI.
- Cancel a long extract/categorize run and resume from persisted checkpoints.
- Confirm the UI still never opens user files, SQLite, or model runtimes.
- Confirm hard bundles cannot be split without an explicit break action.

## Golden path (target)

```text
Setup (once) → Choose source + intent → Scan & analyze (stream) →
Understand the tree (roles) → Review units → Resolve issues →
Preview diff → Confirm Apply → Journaled result / Undo
```

### 1. First-run setup (once, skippable later)

Open a **Setup** view, not a buried dialog. Four independent slots:

| Slot | Job | Typical backend |
|------|-----|-----------------|
| Categorization | Folder/subfolder labels | Local text GGUF, OpenAI, Gemini, custom `/chat/completions` |
| Image description | Visual content + EXIF context | Local visual GGUF + mmproj, or remote vision model |
| Document analysis | PDF/Office text → topic/rename | Text model (may reuse categorization) |
| Assistant chat | Patch proposals from utterances | Same or a cheaper/faster model |

Each slot can be: **Off**, **Download built-in**, **Custom local GGUF**, **OpenAI**,
**Gemini**, **Custom endpoint**. Custom endpoints accept a base URL or a full
chat-completions path, optional API key, and model id. Model files live in a
user-chosen storage directory. GPU/CPU/Vulkan/Metal is a setup concern, not a
per-scan toggle.

The workspace stays usable with all slots Off: heuristics, sidecars, and project
protection still run. Setup must make that explicit.

### 2. Choose source and intent

Left rail stays the start of the path:

- Source path + working Browse dialog (and typed paths).
- Intent cards: **Tidy inbox**, **Build archive**, **Media library**, **Custom**.
- Custom opens Settings instead of silently matching inbox.
- Recent roots persist across restarts.
- A visible stepper: Scan → Review → Resolve → Preview → Apply.

Nothing mutates disk at this step. Copy should say so.

### 3. Scan and analyze, with a live stream

Scan is a staged pipeline, not a spinner:

1. **Walk** — enumerate files/dirs; record skips (hidden, symlink, junk, unreadable).
2. **Folder roles** — classify directories as project, library, archive, inbox, etc.
3. **Relationships** — sidecars, subtitles, archive parts, series, hard links.
4. **Metadata** — media tags, EXIF, document properties (workers).
5. **Content analysis** (if enabled) — vision, document text, then categorization.
6. **Propose** — heuristic + model evidence → a revision.

While this runs:

- A **stage table** shows Scan / Relationships / Metadata / Images / Documents /
  Categorization with counts, current file, elapsed, and cancel.
- A **scrollable stream** (Activity) appends human lines such as
  `photo.CR2 · RAW sidecar of photo.jpg · keep together` and
  `rust-app/ · Rust project · protected`. Power users can catch bad detections early.
- Progress stays visible on every tab, not only Activity.
- Checkpoints persist so a crash or cancel does not throw away already-extracted
  evidence (upstream behaviour: save as you go).

### 4. Understand the tree before file-by-file review

After the snapshot is ready, Structure is a **source + proposed** tree, not ASCII
destinations only. Directories carry a role chip:

| Role | Default action | Example |
|------|----------------|---------|
| Protected project | Do not part out | `rust-app/`, Unity, git worktrees |
| Move as unit (`PreserveLayout`) | Relocate the folder, keep internals | a Blender project the user *wants* archived |
| Media / music library | Keep library layout; maybe rename in place | existing `Music/Artist/Album` |
| Broad folder | Treat children independently | a dump named `Downloads` |
| Weak/partial archive | Offer to reuse path segments | `old/2019/client-a/final` |
| Loose files | Family + sidecar grouping | inbox-mixed photos and clips |

The first review question is about **units**, not files: confirm protected roots,
choose whether weak archives are signal or noise, and keep libraries intact.

### 5. Review units, then exceptions

- Relationships is the bundle/unit list (human labels, not raw JSON constraints).
- Items is a virtualized table of files *and* grouped rows (one row for a
  RAW+JPEG+XMP set). Hard bundles share one Accept/Reject.
- Inspector shows evidence, skipped reason, confidence, and source vs proposed path.
- Filter/search works on source path, destination, family, role, and review state.
- Ambiguous / low-confidence items sort first.

### 6. Resolve, preview, apply

- Validate is automatic after Accept, but the user can still run it.
- Errors are actionable (`keep photo bundle together`, `Documents/readme.txt exists`).
- Preview renders the plan: N moves, N folders created, N protected, N skipped,
  expandable from→to rows (the old dry-run table).
- Apply asks for confirmation (source, counts, undo limits) then journals.
- Activity shows per-operation states. Undo is scoped to that journal and disabled
  when identity checks would be unsafe.

## Design constraints (do not regress)

- UI process never opens user files, SQLite, or model runtimes.
- Workers return evidence only; they never mutate files or the store.
- Hard bundle splits remain plan errors unless the user explicitly breaks the bundle.
- Additive protocol changes; bump `PROTOCOL_VERSION` only for breaking wire changes.
- Secrets (API keys) stay in the engine/os key store, never in renderer logs.

## Current gaps this plan closes

| Gap | Where it lives today | Plan phase |
|-----|----------------------|------------|
| Tabs look like generic buttons | `App.vue` tab nav | 0 |
| Scan/extract progress is throttled, then cleared; Activity is empty during the run | engine emits `progress` every 50 files; UI stores one event and nulls it | 1 |
| `Event::Log` exists and is unused | protocol + engine | 1 |
| Skipped entries never rendered | snapshot.skipped | 1 |
| Existing path context discarded; `PreserveLayout` unused | planner + no folder-role detector | 2 |
| Review is per-file; bundles are display-only | Items table + Relationships cards | 2–3 |
| Preview is a journal UUID | Activity panel | 3 |
| Accept-before-validate is implicit | planner `nothing_accepted` | 3 |
| Apply has no confirmation | footer Apply button | 3 |
| Custom preset promises Settings that do not exist | intent cards | 4 |
| No model download/selection/endpoints | llm worker is a stub | 5–6 |
| Large mixed trees flatten or vanish | family heuristic + strong-project skip | 2, 7 |
| Empty source folders not cleaned | planner never emits `remove_empty_directory` | 3 |
| Recent roots / sessions not persisted | in-memory array | 0 |
| Native folder picker / contrast / select labels | shell CSS | 0 |

## Information architecture

Split the monolith `App.vue` into views and presentational components. Keep engine
calls in `engine.ts`.

```text
src/
  App.vue                         shell: workspace | settings | setup
  views/WorkspaceView.vue
  views/SettingsView.vue
  views/SetupView.vue
  components/
    TabBar.vue                    real tabs (role=tablist)
    WorkflowStepper.vue
    SourceRail.vue
    StageTable.vue
    AnalysisStream.vue            virtualized log
    ScanSummary.vue
    StructureTree.vue             source + dest, virtualized
    ItemsTable.vue                virtualized, grouped rows
    RelationshipsPanel.vue
    PreviewDiff.vue
    ApplyConfirm.vue
    Inspector.vue
    AssistantPane.vue
    FolderRoleCard.vue
    ModelSlotCard.vue
    WhitelistEditor.vue
```

### Tabs

Use a tablist, not a row of identical buttons:

- `role="tablist"` / `role="tab"` / `aria-selected` / `aria-controls`.
- Selected tab is connected to the panel (underline or joined background), not just
  a green border on a chrome-identical button.
- Labels: **Structure**, **Items**, **Relationships**, **Activity** (title case).
- Optional counts: `Items 6`, `Relationships 3`, `Activity 12`.
- Filter input lives inside Items (and Relationships), not the tab bar.

### Surfaces besides the workspace

- **Settings** — classification, scan, and apply policy for this machine/session.
- **Setup** — model/runtime inventory. Linked from Settings and from a status chip
  when a needed slot is Off (`Image analysis is off — set up`).

## Engine and protocol work

### Folder roles

Add a directory-level detector (new module under `aifs-relationships` or
`aifs-scanner`) that runs after the walk and before propose:

- Reuse existing **strong project** rules as `Protected`.
- Add **library** heuristics: many audio files with album/artist tags under a
  stable `Artist/Album` layout; photo libraries with `YYYY/MM` or sidecar density.
- Add **broad folder** heuristics: generic names (`Downloads`, `Desktop`, `Inbox`)
  or high family diversity and low internal structure.
- Add **weak archive** heuristics: dated or numbered path segments, names like
  `old`, `backup`, `final`, `archive`.
- Emit `PreserveLayout` bundles for units the user may relocate without parting
  out, distinct from `Protected`.

Planner changes: path segments become evidence. Default for Tidy inbox:

- Protected → unchanged.
- Libraries → unchanged (or in-place rename only).
- Weak archives → prefer reusing meaningful segments when confidence is high.
- Broad / loose → family folders + sidecar grouping (today’s behaviour).

### Analysis stages and the stream

Today `scan` walks, enriches, and extracts in one request, emitting coarse
`progress` every 50 files and never `log`. Change to:

1. Emit `progress` at stage boundaries *and* on a bounded cadence (time-based,
   e.g. 100ms, not every file — keep the JSONL pipe healthy).
2. Emit `log` lines for notable detections (projects, bundles, skips, worker
   failures, model refusals). Cap and persist; UI virtualizes.
3. Optional richer event (additive) `analysis_item { stage, path, summary, family? }`
   if `log` strings prove too lossy for the table. Prefer extending `progress.message`
   + `log` first to avoid a protocol bump.
4. Persist evidence incrementally in the store during extract/categorize.
5. Honour `cancel` between files; resume from last persisted evidence.

Workers stay isolated. Extend the worker dialect later with `describe` / `categorize`
/ `chat` commands; until then the llm worker remains unused for extract.

### Config commands (additive)

New engine commands, all optional for old clients:

| Command | Purpose |
|---------|---------|
| `get_settings` / `put_settings` | Scan/proposal/analysis policy |
| `get_models` / `put_models` | Slot assignments, endpoint config (keys redacted on get) |
| `download_model` | Built-in GGUF fetch with progress |
| `probe_endpoint` | Validate a custom URL/key without scanning |

Settings live in the engine-owned store (or a sidecar config file the engine owns).
The UI only sends/receives JSON.

### Classification policy (Settings)

Port upstream controls onto `ScanOptions` / `ProposalPolicy` (defaults keep current
behaviour):

- Folder style: consistent vs refined (already exists).
- Category language (canonical English internally, display translation later).
- Named category whitelists: main categories, global subcategories *or* per-category
  branching (mutually exclusive, as upstream).
- Analyze images / documents / media-rename / date prefixes / image-only /
  document-only / rename-only.
- Include hidden, max depth, protect projects, extract metadata.
- Local learning from accepted reviews (after the core path works; store
  accepted (family, tokens) → destination as evidence, never as a silent override
  of hard constraints).

Intent presets become named overlays on this policy. **Custom** means “open
Settings, then scan with those values.”

### Preview, empty dirs, confirmation

- Planner emits `remove_empty_directory` for directories emptied by accepted moves,
  never for pre-existing destinations (apply already refuses to delete those on undo).
- `plan` and dry-run `apply` must be enough for the UI to render a from→to table;
  stop showing only journal ids.
- UI confirmation is not an engine concern; engine still re-checks identities.

## Phased delivery

Each phase should land as a reviewable slice: protocol/engine tests first, then UI
wired to that slice, then a fixture or walkthrough. Do not wait for models to make
the golden path usable.

### Phase 0 — Workspace shell

**Outcome:** The app looks like a workspace with real tabs, a stepper, and durable
source memory. No behaviour change to scan/apply yet except visual/a11y.

Work:

- Split `App.vue`; add TabBar with ARIA tab pattern and title-case labels.
- Stepper in the footer: Scan / Review / Resolve / Preview / Apply, with the
  current step derived from revision/plan/journal state.
- Persist recent roots (engine session list or a small settings blob).
- CSS: readable native `<select>`, contrast, constraint labels as plain English
  (`Protected · do not move members`, `Keep together`).
- Fix Browse (blocking dialog on the Tauri thread is fine; verify it in this
  environment or fall back to a documented typed-path path).
- Empty Custom: clicking it opens Settings (even a stub page) instead of matching
  inbox with a lying hint.

Tests: Vue tab selection; tree tests unchanged.

### Phase 1 — Scan transparency

**Outcome:** A long scan is watchable and cancellable. Skips and projects are
visible. Progress is not cleared before the user sees it.

Work:

- Engine: stage-named progress (`scan`, `relationships`, `extract`, later
  `vision`, `documents`, `categorize`); time-based cadence; `log` for detections
  and skip reasons; keep emitting on cancel boundaries.
- Desktop: subscribe to `progress` *and* `log`; keep a capped ring buffer;
  render StageTable + AnalysisStream on Activity; show a compact stage chip in
  the status bar on every tab; Cancel button bound to `cancel`.
- Scan summary: `N files · N bundles · N projects · N skipped`, with a skipped
  list (reason + path). Protected project interiors appear here, not as missing
  files.
- Do not null `progress` in `finally` until the terminal event is processed and
  the user has a summary.

Tests: engine test that a scan emits multiple stages and at least one log line
for a protected project and a sidecar; UI unit test for stream append/cap.

### Phase 2 — Mixed nested trees

**Outcome:** A root that contains a Rust app, a music folder, a dated “old”
archive, and a flat photo dump is explained as four different units.

Work:

- Folder-role detector + snapshot field `directory_roles` (or bundles with a
  `BundleKind::DirectoryRole` / reuse Project + PreserveLayout + new Library).
- Wire `PreserveLayout` from detectors (not only planner/chat).
- Structure tree: source nesting, role chips, expand/collapse, file counts.
- Relationships: human constraint text; click → members in Inspector.
- Grouped review rows for hard bundles.
- Planner uses roles so Tidy inbox does not flatten a music library or part out
  a project. Weak archives contribute path tokens as evidence.

Fixtures:

- Keep `inbox-mixed`.
- Add `junk-drawer`: nested project + `Music/Artist/Album` + `old/2019/client`
  invoices + flat RAW/JPEG + a symlink + a hidden file + an unreadable dir if
  the OS allows.

Tests: role detection; planner leaves library/project internals in place;
sidecar groups still move together when they sit in a broad folder.

### Phase 3 — Review, preview, apply

**Outcome:** The footer path is understandable and the preview is a real diff.

Work:

- After propose, placements stay `proposed`; Validate explains “approve changes
  first” inline if nothing is accepted (do not only dump `plan_rejected`).
- Accept all becomes **Approve all proposed changes** with counts by unit.
- Auto-plan after approve, still allowing manual Validate.
- PreviewDiff from `plan.operations` (and dry-run journal states).
- Apply confirmation modal; Apply remains disabled on errors.
- Undo uses journal status (completed vs dry-run vs failed).
- Emit `remove_empty_directory` safely.
- Assistant: keep keyword tools until Phase 6; show failures in the chat log;
  chat still invalidates the plan.

Tests: planner empty-dir ops; UI mapping of operations to from→to rows;
apply/undo fixtures unchanged plus one emptied source dir.

### Phase 4 — Settings (classification)

**Outcome:** Custom is real. Upstream classification knobs exist without models.

Work:

- Settings view: scan (hidden, depth, protect projects, extract metadata),
  analysis toggles (images/documents/media rename/date prefixes/modes),
  folder style, whitelist editor (main / global sub / branching, mutually
  exclusive), category language placeholder.
- Persist via `get_settings` / `put_settings`.
- Intent presets write through the same policy object.
- Whitelist is enforced on heuristic folder names *and* later on model output.

Tests: policy serde defaults; whitelist branching exclusivity; refined vs
consistent still covered by existing planner tests.

### Phase 5 — Setup (models and endpoints)

**Outcome:** Users can assign each analysis slot without waiting for llama.cpp
to be production-complete. Missing binaries/models produce a clear CTA.

Work:

- Setup view with four `ModelSlotCard`s, storage directory, backend preference.
- Built-in catalog (Gemma 3 4B IT text/visual, optional others) with download
  progress through `download_model` (hash verify, shared storage). HTTP Range
  resume for the ~2.5 GiB Gemma GGUF is
  [`upstream-model-plan.md`](upstream-model-plan.md) wave 1.
- Custom GGUF + optional mmproj; OpenAI / Gemini / custom endpoint probe.
- Redacted `get_models`. Keys never in renderer storage.
- Status chip on the workspace: which slots are ready / stub / off.
- Until Phase 6, enabling a slot records config only; scan copy says
  “model runtime not connected” rather than silently ignoring it.

Tests: settings round-trip; probe failure codes; download checksum mismatch.

### Phase 6 — Real analysis workers

**Outcome:** Enabled slots actually run, stream, and checkpoint.

Work:

- Worker commands beyond `extract`: `describe` (vision), `extract_text`
  (document), `categorize` (text LLM), `chat` (assistant).
- Engine analysis pipeline calls workers with timeouts, CPU fallback messaging,
  and per-file `log` lines (`IMG_1042.jpg · screenshot · Screenshots/UI`).
  Prompt fit, first `n_gpu_layers` guess, and killing the LLM worker on scan
  cancel are sequenced in [`upstream-model-plan.md`](upstream-model-plan.md)
  (waves 2–4), not as Qt ports.
- Persist evidence as it arrives; skip already-analyzed identities on resume.
- Assistant chat uses the chat slot instead of keyword-only interpret, but
  still may only emit `RevisionPatch`es.
- Prompt-injection: model text is evidence, never trusted as a path or SQL.

Tests: stub workers still pass; supervised extract progress; categorize respects
whitelist; protected members cannot be moved by model output.

### Phase 7 — Scale

**Outcome:** Hundreds of thousands of files do not freeze the WebView or the
JSONL pipe.

Work:

- Do not send the full snapshot to the UI in one `scan_completed` blob for large
  trees. Add paged `list_entries` / `list_bundles` / `list_skipped` (or a
  compact snapshot + lazy inspector fetch). This is the likely protocol bump
  if it cannot stay additive.
- Virtualize Items, Structure, and the Activity stream.
- Windowed scan progress; sampled logs with a “show all” fetch.
- Cancel and resume; session picker for interrupted runs.
- Memory budgets in workers; bounded concurrent describe/categorize.

Tests: synthetic large tree (names only) through CLI; UI renders a page of
10k rows without holding 10k DOM nodes.

### Phase 8 — Hardening

**Outcome:** Edge cases from the UX review have explicit UI, not only engine
errors.

- False-positive project: “Treat as normal folder” → drops protection, re-walks.
- Scan root is itself a project: explain why children were walked anyway.
- Destination occupied / case-only clash: inline rename.
- File changed since scan: skip + offer rescan.
- Cloud placeholders / permission errors: skipped list.
- Undo after the user edited a moved file: refuse with path.
- Accessibility: progress announcements (upstream 1.9.0), contrast, keyboard
  tabs, no unlabeled selects.

## Edge-case matrix (must have UI, not just tests)

| Case | User-visible behaviour |
|------|------------------------|
| Nested strong project | One protected unit; children listed as skipped/protected, not missing |
| Scan root is a git repo | Banner: “this folder is a project; files inside can still be organised” |
| Music library under an inbox | Role = library; not flattened into `Music/` by file |
| `old/2019/client-a/final/*.pdf` | Weak archive; offer reuse of `client-a` / date |
| RAW+JPEG+XMP | One review row; split is a blocking error |
| `clip.mp4` + `clip.en.srt` | Same |
| Split zip parts | Same |
| Two `IMG_1.jpg` in different folders | Separate sidecar groups (parent-scoped) |
| Hidden / symlink / Thumbs.db | Skipped list with reason |
| Unreadable subdirectory | Skip + continue (upstream 1.7.3) |
| Destination exists | Preview error, not apply-time surprise |
| File edited after scan | Identity mismatch; no clobber |
| Empty dirs after flatten | Removed only if emptied by this plan |
| Hard link / duplicate | Relationship shown; no silent second copy |
| Model timeout / GPU OOM | Stream line + CPU fallback prompt (once models exist) |
| Custom endpoint down | Probe error on Setup; scan continues with heuristics |

## Testing strategy

- Keep workspace `cargo test` as the gate for domain/planner/apply/scanner.
- Add fixtures: `junk-drawer` (mixed roles) and a generated wide/deep tree for
  paging.
- Desktop: expand beyond `tree.test.ts` — tab semantics, stream buffer cap,
  from→to rendering, grouped accept.
- CLI: `aifs organize --dry-run` remains the fast golden-path oracle.
- Manual: one mixed-root walkthrough per phase that changes UI (scan stream,
  role chips, preview diff, settings, setup). Do not click Apply on fixtures
  unless the fixture is a copy.
- Do not block Phases 0–4 on live llama.cpp.

## Suggested implementation order inside a phase

1. Domain + protocol types and tests.
2. Engine behaviour + CLI visibility.
3. Desktop wiring + component tests.
4. Fixture / walkthrough update.
5. Docs (`architecture.md`, `protocol.md`, this file’s checklist).

## Out of scope until the path is solid

- Full i18n catalogs (keep strings structured so they can be extracted).
- Microsoft Store / CUDA packaging matrix (ggml SSE4.2 / macOS rpath when a
  llama sidecar ships: [`upstream-model-plan.md`](upstream-model-plan.md) wave 5).
- Learning-from-reviews as a silent recategorizer.
- In-process fallback of the engine into the UI.

Local model download resume (Setup, Phase 5) is wave 1 of that same plan.

## Checklist against the UX review

- [ ] Tabs read as tabs (Phase 0)
- [ ] Live analysis stream + stage table (Phase 1, 6)
- [ ] Skipped / protected interiors visible (Phase 1)
- [ ] Folder roles and PreserveLayout (Phase 2)
- [ ] Grouped review, not file-only (Phase 2–3)
- [ ] Human preview diff + Apply confirm + empty-dir cleanup (Phase 3)
- [ ] Settings classification (Phase 4)
- [ ] Setup models/endpoints per slot (Phase 5)
- [ ] Large mixed trees without flattening libraries/projects (Phase 2, 7)
- [ ] Cancel, checkpoint, virtualize (Phase 1, 6, 7)
