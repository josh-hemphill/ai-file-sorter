import assert from "node:assert/strict";
import test from "node:test";
import {
  applyWhitelistMode,
  branchingToText,
  linesToList,
  settingsPageStatus,
  textToBranching,
  whitelistMode,
} from "./settings.ts";

test("linesToList trims and dedupes", () => {
  assert.deepEqual(linesToList("Documents\n\nPictures\ndocuments\n"), [
    "Documents",
    "Pictures",
  ]);
});

test("branching text round-trips", () => {
  const text = "Documents: Reports, Notes\nPictures: Screenshots";
  const map = textToBranching(text);
  assert.deepEqual(map.Documents, ["Reports", "Notes"]);
  assert.equal(branchingToText(map), text);
});

test("settingsPageStatus shows loading until the form is bound", () => {
  assert.equal(settingsPageStatus(false, true, null), "loading");
  assert.equal(settingsPageStatus(false, false, null), "loading");
  assert.equal(settingsPageStatus(false, false, "engine is not running"), "error");
  assert.equal(settingsPageStatus(true, true, null), "saving");
  assert.equal(settingsPageStatus(true, false, null), "ready");
  assert.equal(settingsPageStatus(true, false, "put failed"), "error");
});

test("applyWhitelistMode keeps subcategory styles exclusive", () => {
  const whitelist = {
    main: ["Documents"],
    global_subcategories: ["Reports"],
    branching: { Documents: ["Notes"] },
  };
  const global = applyWhitelistMode(whitelist, "global", "Reports\nInvoices", "");
  assert.deepEqual(global.global_subcategories, ["Reports", "Invoices"]);
  assert.deepEqual(global.branching, {});
  const branching = applyWhitelistMode(whitelist, "branching", "Reports", "Documents: Notes");
  assert.deepEqual(branching.global_subcategories, []);
  assert.deepEqual(branching.branching, { Documents: ["Notes"] });
  assert.equal(whitelistMode(global), "global");
  assert.equal(whitelistMode(branching), "branching");
});
