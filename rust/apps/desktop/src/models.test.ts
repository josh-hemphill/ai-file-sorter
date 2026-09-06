import assert from "node:assert/strict";
import test from "node:test";
import { inventorySummary, slotBackend, withSlotKind } from "./models.ts";

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
        { id: "categorize", kind: "open_ai", model: "gpt-4.1-mini" },
        { id: "vision", kind: "off" },
      ],
    }),
    "1 slot assigned · model runtime not connected",
  );
});

test("withSlotKind clears foreign fields", () => {
  const next = withSlotKind(
    { id: "vision", kind: "open_ai", model: "x", api_key_set: true },
    "catalog",
  );
  assert.equal(next.kind, "catalog");
  assert.equal(next.model, undefined);
  assert.equal(next.api_key, "");
  assert.equal(next.api_key_set, false);
  assert.deepEqual(slotBackend(next), { kind: "catalog", catalog_id: "gemma-3-4b-it" });
});
