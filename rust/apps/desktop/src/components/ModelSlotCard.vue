<script setup lang="ts">
import { mdiCheckCircleOutline, mdiDownloadOutline } from "@mdi/js";
import { computed, ref, watch } from "vue";
import { probeEndpoint } from "../engine";
import { BUILTIN_CATALOG, slotBackend, withSlotKind } from "../models";
import type { ModelBackend, ModelSlot } from "../types";
import Icon from "./Icon.vue";

const props = defineProps<{
  assignment: ModelSlot;
  label: string;
  hint: string;
  downloaded?: boolean;
  downloading?: boolean;
  busy?: boolean;
}>();

const emit = defineEmits<{
  change: [slot: ModelSlot];
  download: [catalogId: string];
}>();

const probeMessage = ref<string | null>(null);
const probing = ref(false);
const apiKey = ref("");

watch(
  () => props.assignment.api_key,
  (key) => {
    if (!key) {
      apiKey.value = "";
    }
  },
);

const kind = computed(() => props.assignment.kind ?? "off");

function update(patch: Partial<ModelSlot>) {
  emit("change", { ...props.assignment, ...patch });
}

function setKind(next: ModelBackend["kind"]) {
  apiKey.value = "";
  probeMessage.value = null;
  emit("change", withSlotKind(props.assignment, next));
}

async function probe() {
  probing.value = true;
  probeMessage.value = null;
  try {
    const [ok, message] = await probeEndpoint(
      slotBackend(props.assignment),
      apiKey.value || undefined,
    );
    probeMessage.value = ok ? message : `Could not probe: ${message}`;
  } catch (error) {
    probeMessage.value = String(error);
  } finally {
    probing.value = false;
  }
}

function requestDownload() {
  const catalogId = props.assignment.catalog_id;
  if (!catalogId) {
    return;
  }
  emit("download", catalogId);
}
</script>

<template>
  <article class="card">
    <strong>{{ label }}</strong>
    <span class="muted">{{ hint }}</span>
    <label class="field">
      Backend
      <select :value="kind" @change="setKind(($event.target as HTMLSelectElement).value as ModelBackend['kind'])">
        <option value="off">Off</option>
        <option value="catalog">Download built-in</option>
        <option value="local_gguf">Custom local GGUF</option>
        <option value="open_ai">OpenAI</option>
        <option value="gemini">Gemini</option>
        <option value="custom_endpoint">Custom endpoint</option>
      </select>
    </label>
    <label v-if="kind === 'catalog'" class="field">
      Catalog
      <select
        :value="assignment.catalog_id"
        @change="update({ catalog_id: ($event.target as HTMLSelectElement).value })"
      >
        <option v-for="item in BUILTIN_CATALOG" :key="item.id" :value="item.id">
          {{ item.label }}
        </option>
      </select>
    </label>
    <template v-if="kind === 'local_gguf'">
      <label class="field">
        GGUF path
        <input
          :value="assignment.path"
          placeholder="/models/model.gguf"
          @input="update({ path: ($event.target as HTMLInputElement).value })"
        />
      </label>
      <label class="field">
        mmproj (optional)
        <input
          :value="assignment.mmproj"
          placeholder="/models/mmproj.gguf"
          @input="update({ mmproj: ($event.target as HTMLInputElement).value || undefined })"
        />
      </label>
    </template>
    <label v-if="kind === 'open_ai' || kind === 'gemini' || kind === 'custom_endpoint'" class="field">
      Model id
      <input
        :value="assignment.model"
        placeholder="gpt-4.1-mini"
        @input="update({ model: ($event.target as HTMLInputElement).value })"
      />
    </label>
    <label v-if="kind === 'custom_endpoint'" class="field">
      Endpoint URL
      <input
        :value="assignment.base_url"
        placeholder="https://example.com/v1"
        @input="update({ base_url: ($event.target as HTMLInputElement).value })"
      />
    </label>
    <label
      v-if="kind === 'open_ai' || kind === 'gemini' || kind === 'custom_endpoint'"
      class="field"
    >
      API key
      <input
        v-model="apiKey"
        type="password"
        autocomplete="off"
        :placeholder="assignment.api_key_set ? 'Key stored in the engine' : 'Optional'"
        @change="update({ api_key: apiKey })"
      />
    </label>
    <div class="row">
      <button
        v-if="kind === 'catalog'"
        type="button"
        class="primary"
        :disabled="busy || downloading || !assignment.catalog_id"
        @click="requestDownload"
      >
        <Icon :path="downloaded ? mdiCheckCircleOutline : mdiDownloadOutline" :size="18" />
        {{ downloaded ? "Already downloaded" : downloading ? "Downloading…" : "Download now" }}
      </button>
      <button type="button" :disabled="probing || kind === 'off' || busy" @click="probe">
        Probe
      </button>
      <span v-if="probeMessage" class="muted">{{ probeMessage }}</span>
    </div>
  </article>
</template>
