import type { ModelBackend, ModelInventory, ModelSlot } from "./types";

export const MODEL_SLOT_META = [
  {
    id: "categorize",
    label: "Categorization",
    hint: "Folder and subcategory labels from a text model.",
  },
  {
    id: "vision",
    label: "Image description",
    hint: "Visual content plus EXIF context.",
  },
  {
    id: "document",
    label: "Document analysis",
    hint: "PDF and Office text for topics and rename suggestions.",
  },
  {
    id: "chat",
    label: "Assistant chat",
    hint: "Natural-language patches. Heuristic tools still work when this is off.",
  },
] as const;

export const BUILTIN_CATALOG = [
  { id: "gemma-3-4b-it", label: "Gemma 3 4B Instruct (text)" },
  { id: "gemma-3-4b-it-mmproj", label: "Gemma 3 4B Instruct + mmproj (vision)" },
] as const;

export const GPU_PREFERENCES = ["auto", "cpu", "vulkan", "metal"] as const;

/** Default engine inventory for an empty Setup form. */
export function defaultInventory(): ModelInventory {
  return {
    storage_dir: "",
    gpu_preference: "auto",
    slots: MODEL_SLOT_META.map((slot) => ({ id: slot.id, kind: "off" as const })),
  };
}

/** Reads the flattened backend from a slot row. */
export function slotBackend(slot: ModelSlot): ModelBackend {
  switch (slot.kind) {
    case "catalog":
      return { kind: "catalog", catalog_id: slot.catalog_id ?? "" };
    case "local_gguf":
      return { kind: "local_gguf", path: slot.path ?? "", mmproj: slot.mmproj };
    case "open_ai":
      return { kind: "open_ai", model: slot.model ?? "" };
    case "gemini":
      return { kind: "gemini", model: slot.model ?? "" };
    case "custom_endpoint":
      return {
        kind: "custom_endpoint",
        base_url: slot.base_url ?? "",
        model: slot.model ?? "",
      };
    default:
      return { kind: "off" };
  }
}

/** Applies a backend kind, clearing fields that belong to other kinds. */
export function withSlotKind(slot: ModelSlot, kind: ModelBackend["kind"]): ModelSlot {
  return {
    id: slot.id,
    kind,
    api_key_set: slot.api_key_set,
    catalog_id: kind === "catalog" ? (slot.catalog_id ?? BUILTIN_CATALOG[0].id) : undefined,
    path: kind === "local_gguf" ? (slot.path ?? "") : undefined,
    mmproj: kind === "local_gguf" ? slot.mmproj : undefined,
    model:
      kind === "open_ai" || kind === "gemini" || kind === "custom_endpoint"
        ? (slot.model ?? "")
        : undefined,
    base_url: kind === "custom_endpoint" ? (slot.base_url ?? "") : undefined,
  };
}

/** Compact workspace chip copy. */
export function inventorySummary(inventory: ModelInventory): string {
  const assigned = inventory.slots.filter((slot) => slot.kind && slot.kind !== "off").length;
  if (assigned === 0) {
    return "All analysis slots off";
  }
  return `${assigned} slot${assigned === 1 ? "" : "s"} assigned · model runtime not connected`;
}
