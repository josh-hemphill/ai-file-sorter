import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

test("pickFolder awaits the async pick_folder command", () => {
  const src = readFileSync(new URL("./engine.ts", import.meta.url), "utf8");
  assert.match(src, /export async function pickFolder/);
  assert.match(src, /return invoke\("pick_folder"\)/);
});
