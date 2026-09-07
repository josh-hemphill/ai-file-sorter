import assert from "node:assert/strict";
import test from "node:test";
import {
  catalogIsDownloaded,
  formatBytes,
  inventorySummary,
  slotBackend,
  withSlotKind,
} from "./models.ts";

test("inventorySummary treats off slots as unused", () => {
  assert.equal(
    inventorySummary({
      storage_dir: "",
      gpu_preference: "auto",
      slots: [
        { id: "categorize", kind: "off" },
        { id: "vision", kind: "off" },
        { id: "document", kind: "off" },
        { id: "chat", kind: "off" },
      ],
    }),
    "All analysis slots off",
  );
  assert.equal(
    inventorySummary({
      storage_dir: "",
      gpu_preference: "auto",
      slots: [
        { id: "categorize", kind: "open_ai", model: "gpt-4.1-mini", runtime: { kind: "hosted", detail: "hosted" } },
        { id: "vision", kind: "off", runtime: { kind: "off", detail: "off" } },
        { id: "document", kind: "off", runtime: { kind: "off", detail: "off" } },
        { id: "chat", kind: "off", runtime: { kind: "off", detail: "off" } },
      ],
    }),
    "1 hosted · 3 off",
  );
});

test("inventorySummary does not claim heuristics when infer is stub or llama", () => {
  assert.equal(
    inventorySummary({
      storage_dir: "",
      gpu_preference: "auto",
      slots: [
        { id: "categorize", kind: "catalog", catalog_id: "gemma-3-4b-it", runtime: { kind: "stub", detail: "stub" } },
        { id: "vision", kind: "catalog", catalog_id: "gemma-3-4b-it", runtime: { kind: "stub", detail: "stub" } },
        { id: "document", kind: "off", runtime: { kind: "off", detail: "off" } },
        { id: "chat", kind: "off", runtime: { kind: "off", detail: "off" } },
      ],
    }),
    "2 stub · 2 off",
  );
  assert.equal(
    inventorySummary({
      storage_dir: "",
      gpu_preference: "auto",
      slots: [
        { id: "categorize", kind: "catalog", catalog_id: "gemma-3-4b-it", runtime: { kind: "llama", detail: "llama" } },
        { id: "vision", kind: "local_gguf", path: "/missing.gguf", runtime: { kind: "missing_files", detail: "missing" } },
        { id: "document", kind: "open_ai", model: "x", runtime: { kind: "missing_worker", detail: "missing" } },
        { id: "chat", kind: "off", runtime: { kind: "off", detail: "off" } },
      ],
    }),
    "1 llama · 1 no worker · 1 missing files · 1 off",
  );
  assert.equal(
    inventorySummary({
      storage_dir: "",
      gpu_preference: "auto",
      slots: [
        { id: "categorize", kind: "open_ai", model: "gpt-4.1-mini" },
        { id: "vision", kind: "off" },
      ],
    }),
    "1 assigned · 1 off",
  );
});

test("withSlotKind clears foreign fields", () => {
  const next = withSlotKind(
    {
      id: "vision",
      kind: "open_ai",
      model: "x",
      api_key_set: true,
      runtime: { kind: "hosted", detail: "hosted" },
    },
    "catalog",
  );
  assert.equal(next.kind, "catalog");
  assert.equal(next.model, undefined);
  assert.equal(next.api_key, undefined);
  assert.equal(next.api_key_set, false);
  assert.deepEqual(slotBackend(next), { kind: "catalog", catalog_id: "gemma-3-4b-it" });
  assert.equal(next.runtime, undefined);
});

test("withSlotKind round-trip to hosted omits a blank key", () => {
  const off = withSlotKind(
    { id: "vision", kind: "open_ai", model: "x", api_key_set: true },
    "off",
  );
  const hosted = withSlotKind(off, "open_ai");
  assert.equal(hosted.api_key, undefined);
});

test("catalogIsDownloaded is true only when every shared file is present", () => {
  const artifacts = [
    {
      id: "gemma-text-q4",
      filename: "google_gemma-3-4b-it-Q4_K_M.gguf",
      path: "/models/google_gemma-3-4b-it-Q4_K_M.gguf",
      expected_bytes: 2_490_000_000,
      bytes_on_disk: 2_490_000_000,
      present: true,
      used_by: ["gemma-3-4b-it", "gemma-3-4b-it-mmproj"],
    },
    {
      id: "gemma-mmproj-f16",
      filename: "mmproj-google_gemma-3-4b-it-f16.gguf",
      path: "/models/mmproj-google_gemma-3-4b-it-f16.gguf",
      expected_bytes: 851_000_000,
      bytes_on_disk: 0,
      present: false,
      used_by: ["gemma-3-4b-it-mmproj"],
    },
  ];
  assert.equal(
    catalogIsDownloaded({ storage_dir: "/models", gpu_preference: "auto", slots: [], artifacts }, "gemma-3-4b-it"),
    true,
  );
  assert.equal(
    catalogIsDownloaded(
      { storage_dir: "/models", gpu_preference: "auto", slots: [], artifacts },
      "gemma-3-4b-it-mmproj",
    ),
    false,
  );
  assert.equal(formatBytes(2_490_000_000), "2.49 GB");
});
