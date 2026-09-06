<script setup lang="ts">
import { onMounted, ref } from "vue";
import ModelSlotCard from "../components/ModelSlotCard.vue";
import { connectEngine, getModels, putModels } from "../engine";
import {
  defaultInventory,
  GPU_PREFERENCES,
  MODEL_SLOT_META,
} from "../models";
import type { ModelInventory, ModelSlot } from "../types";

const emit = defineEmits<{
  back: [];
}>();

const inventory = ref<ModelInventory>(defaultInventory());
const error = ref<string | null>(null);
const saved = ref(false);
const busy = ref(false);

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

onMounted(() => {
  void load();
});
</script>

<template>
  <section class="settings">
    <header class="settings-head">
      <div>
        <h1>Setup</h1>
        <p class="muted">
          Each analysis slot can be off, a downloaded local model, or a remote/custom endpoint.
          The workspace stays usable with every slot off. Keys are stored by the engine, not
          in this window. Assignments are recorded now; the model runtime is not connected yet.
        </p>
      </div>
      <div class="row">
        <button type="button" :disabled="busy" @click="save">Save</button>
        <button type="button" class="primary" @click="emit('back')">Back to workspace</button>
      </div>
    </header>
    <p v-if="error" class="error">{{ error }}</p>
    <p v-else-if="saved" class="muted">Saved. Scan still uses heuristics until workers exist.</p>

    <article class="card">
      <h2>Storage</h2>
      <label class="field">
        Model directory
        <input v-model="inventory.storage_dir" placeholder="/path/to/models" />
      </label>
      <label class="field">
        Accelerator
        <select v-model="inventory.gpu_preference">
          <option v-for="item in GPU_PREFERENCES" :key="item" :value="item">{{ item }}</option>
        </select>
      </label>
    </article>

    <ModelSlotCard
      v-for="slot in inventory.slots"
      :key="slot.id"
      :assignment="slot"
      :label="metaFor(slot.id).label"
      :hint="metaFor(slot.id).hint"
      @change="replaceSlot"
    />
  </section>
</template>
