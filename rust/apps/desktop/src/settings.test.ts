import assert from "node:assert/strict";
import test from "node:test";
import {
  applyWhitelistMode,
  branchingToText,
  linesToList,
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
