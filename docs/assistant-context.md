# Assistant context

Chat never receives SQL, raw filesystem operations, or the full snapshot JSON.
The engine builds a **paged prompt** from the current snapshot and revision, then
the worker replies with `{"message","patches"}`. Keyword tools (`search`,
`inspect`, `structure`, `group`, `rename`, `validate`) still run when the chat
slot is off or the model reply is not parseable.

## What the model sees

`chat_context` is capped at 3500 characters. It is a page, not the whole tree:

1. **Instructions** — JSON patch vocabulary; root-relative paths only; do not
   patch members of a layout unit. The engine drops those destination edits.
2. **Units** — strong projects and outermost `PreserveLayout` folders (libraries
   and weak archives). Each line has the root, why it is a unit, file count, and
   a few immediate child names. Nested albums/date folders are *not* listed as
   extra units; they inherit the parent.
3. **Loose files** — files that are *not* inside a deferred unit. Each line is
   `id path destination · category · description`. Descriptions are truncated.
   Files whose path or evidence matches tokens in the utterance are ranked first
   (up to 48 files).

Protected project internals are not in the snapshot at all (the scanner skips
them). Layout-unit members stay in the snapshot so the planner can move the
folder as a whole, but they are omitted from the loose-file page to save tokens
and to stop the model from describing/sorting them one by one.

## How to iterate

The first prompt is intentionally incomplete. Later turns should:

- **Inspect a unit** — keyword `inspect` / `keep` already summarises a bundle.
  Chat cannot yet break a library into independently organised files; that still
  needs an explicit follow-up (no re-describe pass is wired).
- **Search** — `find` / `search` looks through paths and evidence without stuffing
  the whole tree into the prompt.
- **Reuse the utterance** — tokens from the current chat line re-rank the loose
  file page, so “move the Padmé cues” surfaces matching names even when they
  were not in the first 48 files of a large inbox.

There is no model-side tool loop yet. The engine does retrieval *before* the
worker call. Adding JSON tool calls (`inspect_unit`, `list_files {prefix,cursor}`)
would be the next step if paged context is still too coarse.

## Scan-time deferral

Describe and categorize skip files inside a layout unit and log

`Pictures · deferred · 133 files stay in this folder as a unit`.

Extract (tags/EXIF) still runs so unit summaries and later inspect/search have
facts. Uncheck **Reuse previous analysis** (or pass `--fresh`) when you want
those workers to run again from scratch.
