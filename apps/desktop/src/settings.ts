import type { AppSettings, CategoryWhitelist } from "./types";

export type WhitelistMode = "none" | "global" | "branching";

export type SettingsPageStatus = "loading" | "saving" | "error" | "ready";

/** Visible settings-page phase while engine I/O is in flight. */
export function settingsPageStatus(
  loaded: boolean,
  busy: boolean,
  error: string | null,
): SettingsPageStatus {
  if (error) {
    return "error";
  }
  if (!loaded) {
    return "loading";
  }
  if (busy) {
    return "saving";
  }
  return "ready";
}

/** Default engine settings for an empty form. */
export function defaultSettings(): AppSettings {
  return {
    scan: {
      recursive: true,
      max_depth: 0,
      include_hidden: false,
      protect_projects: true,
      extract_metadata: true,
      fingerprint_prefix_bytes: 64 * 1024,
    },
    policy: {
      style: "consistent",
      use_subfolders: true,
      rename_media: true,
      rename_images_with_date: false,
      pinned_families: ["code"],
      project_folder: null,
      whitelist: { main: [], global_subcategories: [], branching: {} },
      category_language: "en",
    },
    analyze_images: false,
    analyze_documents: false,
  };
}

/** Splits a textarea into unique trimmed lines. */
export function linesToList(text: string): string[] {
  const seen = new Set<string>();
  const list: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    const item = line.trim();
    if (!item || seen.has(item.toLowerCase())) {
      continue;
    }
    seen.add(item.toLowerCase());
    list.push(item);
  }
  return list;
}

/** Joins a string list for a textarea. */
export function listToLines(list: string[]): string {
  return list.join("\n");
}

/** Parses `Category: child, child` lines into a branching map. */
export function textToBranching(text: string): Record<string, string[]> {
  const branching: Record<string, string[]> = {};
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) {
      continue;
    }
    const split = trimmed.indexOf(":");
    if (split <= 0) {
      continue;
    }
    const category = trimmed.slice(0, split).trim();
    const children = trimmed
      .slice(split + 1)
      .split(",")
      .map((child) => child.trim())
      .filter(Boolean);
    if (category) {
      branching[category] = children;
    }
  }
  return branching;
}

/** Serializes a branching map for the editor. */
export function branchingToText(branching: Record<string, string[]>): string {
  return Object.entries(branching)
    .map(([category, children]) => `${category}: ${children.join(", ")}`)
    .join("\n");
}

/** Active subcategory style, preferring global when both are empty. */
export function whitelistMode(whitelist: CategoryWhitelist): WhitelistMode {
  if (whitelist.global_subcategories.length > 0) {
    return "global";
  }
  if (Object.keys(whitelist.branching).length > 0) {
    return "branching";
  }
  return "none";
}

/** Applies the selected subcategory mode so the two styles stay exclusive. */
export function applyWhitelistMode(
  whitelist: CategoryWhitelist,
  mode: WhitelistMode,
  globalText: string,
  branchingText: string,
): CategoryWhitelist {
  if (mode === "global") {
    return {
      ...whitelist,
      global_subcategories: linesToList(globalText),
      branching: {},
    };
  }
  if (mode === "branching") {
    return {
      ...whitelist,
      global_subcategories: [],
      branching: textToBranching(branchingText),
    };
  }
  return { ...whitelist, global_subcategories: [], branching: {} };
}
