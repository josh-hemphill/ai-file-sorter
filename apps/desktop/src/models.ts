import type { ModelArtifactStatus, ModelBackend, ModelInventory, ModelSlot } from "./types";

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
    hint: "Model JSON patches when this slot is on; keyword tools still run if the slot is off or the model does not emit patches.",
  },
] as const;

export const BUILTIN_CATALOG = [
  { id: "gemma-3-4b-it", label: "Gemma 3 4B Instruct (text)" },
  { id: "gemma-3-4b-it-mmproj", label: "Gemma 3 4B Instruct + mmproj (vision)" },
] as const;

export const GPU_PREFERENCES = [
  {
    id: "auto",
    label: "Auto",
    hint: "Prefer CUDA, then Vulkan or Metal, then CPU, in a llama.cpp worker built with those features.",
  },
  { id: "cpu", label: "CPU only", hint: "No GPU offload." },
  {
    id: "cuda",
    label: "CUDA",
    hint: "Needs NVIDIA drivers and `aifs-worker-llm` built with `--features llama,cuda`.",
  },
  {
    id: "vulkan",
    label: "Vulkan",
    hint: "Needs `aifs-worker-llm` built with `--features llama,vulkan`.",
  },
  {
    id: "metal",
    label: "Metal",
    hint: "Needs `aifs-worker-llm` built with `--features llama,metal` on macOS.",
  },
] as const;

/** Default engine inventory for an empty Setup form. */
export function defaultInventory(): ModelInventory {
  return {
    storage_dir: "",
    gpu_preference: "auto",
    slots: MODEL_SLOT_META.map((slot) => ({ id: slot.id, kind: "off" as const })),
    artifacts: [],
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
  const hosted = kind === "open_ai" || kind === "gemini" || kind === "custom_endpoint";
  return {
    id: slot.id,
    kind,
    api_key: hosted ? slot.api_key : undefined,
    api_key_set: hosted ? slot.api_key_set : false,
    catalog_id: kind === "catalog" ? (slot.catalog_id ?? BUILTIN_CATALOG[0].id) : undefined,
    path: kind === "local_gguf" ? (slot.path ?? "") : undefined,
    mmproj: kind === "local_gguf" ? slot.mmproj : undefined,
    model: hosted ? (slot.model ?? "") : undefined,
    base_url: kind === "custom_endpoint" ? (slot.base_url ?? "") : undefined,
  };
}

/** Compact workspace chip copy. */
export function inventorySummary(inventory: ModelInventory): string {
  const assigned = inventory.slots.filter((slot) => slot.kind && slot.kind !== "off").length;
  if (assigned === 0) {
    return "All analysis slots off";
  }
  return `${assigned} slot${assigned === 1 ? "" : "s"} assigned · scan still uses heuristics`;
}

/** Human size for catalog files. */
export function formatBytes(bytes: number): string {
  if (bytes >= 1_000_000_000) {
    return `${(bytes / 1_000_000_000).toFixed(2)} GB`;
  }
  if (bytes >= 1_000_000) {
    return `${(bytes / 1_000_000).toFixed(1)} MB`;
  }
  if (bytes >= 1_000) {
    return `${(bytes / 1_000).toFixed(0)} KB`;
  }
  return `${bytes} B`;
}

/** True when every GGUF this catalog id needs is already on disk. */
export function catalogIsDownloaded(inventory: ModelInventory, catalogId: string): boolean {
  const needed = (inventory.artifacts ?? []).filter((artifact) =>
    artifact.used_by.includes(catalogId),
  );
  return needed.length > 0 && needed.every((artifact) => artifact.present);
}

/** Artifacts the Setup page lists once, even when several slots share them. */
export function catalogArtifacts(inventory: ModelInventory): ModelArtifactStatus[] {
  return inventory.artifacts ?? [];
}
