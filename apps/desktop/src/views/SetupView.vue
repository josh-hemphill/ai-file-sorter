<script setup lang="ts">
import {
  mdiArrowLeft,
  mdiCheckCircleOutline,
  mdiContentSaveOutline,
  mdiDownloadOutline,
  mdiInformationOutline,
} from "@mdi/js";
import { computed, onMounted, onUnmounted, ref } from "vue";
import Icon from "../components/Icon.vue";
import ModelSlotCard from "../components/ModelSlotCard.vue";
import { connectEngine, downloadModel, getModels, onEngineProgress, putModels } from "../engine";
import {
  catalogArtifacts,
  catalogIsDownloaded,
  defaultInventory,
  formatBytes,
  GPU_PREFERENCES,
  MODEL_SLOT_META,
  withPresentedRuntime,
} from "../models";
import type { ModelInventory, ModelSlot, ProgressEvent } from "../types";

const emit = defineEmits<{
  back: [];
}>();

const inventory = ref<ModelInventory>(defaultInventory());
const error = ref<string | null>(null);
const saved = ref(false);
const busy = ref(false);
const loaded = ref(false);
const downloadingId = ref<string | null>(null);
const downloadProgress = ref<ProgressEvent | null>(null);
let stopProgress: (() => void) | undefined;

const gpuHint = computed(() => {
  return GPU_PREFERENCES.find((item) => item.id === inventory.value.gpu_preference)?.hint ?? "";
});

const artifacts = computed(() => catalogArtifacts(inventory.value));

const downloadPercent = computed(() => {
  const progress = downloadProgress.value;
  if (!progress?.total || progress.total === 0) {
    return 0;
  }
  return Math.min(100, Math.round((progress.current / progress.total) * 100));
});

function metaFor(id: string) {
  return MODEL_SLOT_META.find((slot) => slot.id === id) ?? {
    id,
    label: id,
    hint: "",
  };
}

function replaceSlot(next: ModelSlot) {
  inventory.value = {
    ...inventory.value,
    slots: inventory.value.slots.map((slot) => (slot.id === next.id ? next : slot)),
  };
}

async function load() {
  busy.value = true;
  error.value = null;
  try {
    await connectEngine();
    inventory.value = await getModels();
    loaded.value = true;
  } catch (cause) {
    error.value = String(cause);
  } finally {
    busy.value = false;
  }
}

async function save() {
  busy.value = true;
  error.value = null;
  saved.value = false;
  try {
    inventory.value = await putModels(inventory.value);
    saved.value = true;
  } catch (cause) {
    error.value = String(cause);
  } finally {
    busy.value = false;
  }
}

async function download(catalogId: string) {
  busy.value = true;
  error.value = null;
  downloadingId.value = catalogId;
  downloadProgress.value = {
    stage: "download",
    current: 0,
    total: null,
    message: "Starting download…",
  };
  try {
    const result = await downloadModel(catalogId);
    inventory.value = withPresentedRuntime(inventory.value, result);
  } catch (cause) {
    error.value = String(cause);
  } finally {
    busy.value = false;
    downloadingId.value = null;
  }
}

onMounted(() => {
  void load();
  void onEngineProgress((event) => {
    if (event.stage === "download") {
      downloadProgress.value = event;
    }
  }).then((stop) => {
    stopProgress = stop;
  });
});

onUnmounted(() => {
  stopProgress?.();
});
</script>

<template>
  <section class="settings page" aria-labelledby="setup-title">
    <nav class="crumb" aria-label="Breadcrumb">
      <button type="button" class="crumb-link" @click="emit('back')">Workspace</button>
      <span aria-hidden="true">/</span>
      <span aria-current="page">Setup</span>
    </nav>
    <header class="settings-head">
      <div>
        <h1 id="setup-title">Setup</h1>
        <p class="muted">
          This page records which local or remote model each analysis slot should use. Saving
          does not start a model. Scan loads the LLM worker for assigned slots. From-source
          `make desktop` / `cargo engine-llm` compile llama.cpp into that worker; `cargo test`
          still uses the stub. CUDA/Vulkan/Metal stay opt-in features. Propose stays heuristic
          if load fails. Several slots can share one downloaded GGUF — it is fetched once.
        </p>
      </div>
      <div class="row">
        <button type="button" :disabled="busy || !loaded" @click="save">
          <Icon :path="mdiContentSaveOutline" :size="18" />
          Save assignments
        </button>
        <button type="button" class="primary" @click="emit('back')">
          <Icon :path="mdiArrowLeft" :size="18" />
          Back to workspace
        </button>
      </div>
    </header>
    <p v-if="error" class="error">{{ error }}</p>
    <p v-else-if="saved" class="ok">
      Assignments saved. Scan will load the LLM worker for assigned slots. From-source
      `make desktop` compiles llama.cpp; `cargo test` still uses the stub worker.
    </p>

    <article class="card">
      <h2>Storage</h2>
      <label class="field">
        Model directory
        <input v-model="inventory.storage_dir" placeholder="/path/to/models" />
        <span class="muted">Leave blank to use the engine default (`…/aifs/models`).</span>
      </label>
      <label class="field">
        Accelerator
        <select v-model="inventory.gpu_preference">
          <option v-for="item in GPU_PREFERENCES" :key="item.id" :value="item.id">
            {{ item.label }}
          </option>
        </select>
        <span class="muted">{{ gpuHint }}</span>
      </label>
      <p class="callout">
        <Icon :path="mdiInformationOutline" :size="18" />
        CUDA lives only in `aifs-worker-llm`. Build that worker with `--features llama,cuda`,
        install NVIDIA drivers, and set this accelerator to CUDA (or Auto, which prefers CUDA).
        The UI and engine never load CUDA.
      </p>
    </article>

    <article class="card">
      <h2>Downloaded files</h2>
      <p class="muted">
        Catalog slots share these GGUF files. Download now fetches only what is missing.
      </p>
      <ul class="artifact-list">
        <li v-for="artifact in artifacts" :key="artifact.id">
          <div class="artifact-head">
            <Icon
              :path="artifact.present ? mdiCheckCircleOutline : mdiDownloadOutline"
              :size="18"
            />
            <strong>{{ artifact.filename }}</strong>
            <span v-if="artifact.present" class="badge ok-badge">Already downloaded</span>
            <span v-else class="badge">Not downloaded</span>
          </div>
          <p class="muted">
            {{
              artifact.present
                ? formatBytes(artifact.bytes_on_disk)
                : `about ${formatBytes(artifact.expected_bytes)}`
            }}
            · used by {{ artifact.used_by.join(", ") }}
          </p>
        </li>
      </ul>
      <p v-if="!artifacts.length" class="muted">Connect the engine to see catalog files.</p>
      <div v-if="downloadingId" class="download-progress">
        <progress :value="downloadPercent" max="100" />
        <span class="muted">{{ downloadProgress?.message }} ({{ downloadPercent }}%)</span>
      </div>
    </article>

    <ModelSlotCard
      v-for="slot in inventory.slots"
      :key="slot.id"
      :assignment="slot"
      :label="metaFor(slot.id).label"
      :hint="metaFor(slot.id).hint"
      :downloaded="catalogIsDownloaded(inventory, slot.catalog_id ?? '')"
      :downloading="downloadingId === (slot.catalog_id ?? '')"
      :busy="busy"
      @change="replaceSlot"
      @download="download"
    />
  </section>
</template>
