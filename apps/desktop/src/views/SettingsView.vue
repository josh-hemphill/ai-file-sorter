<script setup lang="ts">
import { mdiArrowLeft, mdiContentSaveOutline, mdiDownloadOutline } from "@mdi/js";
import { onMounted, ref } from "vue";
import Icon from "../components/Icon.vue";
import WhitelistEditor from "../components/WhitelistEditor.vue";
import { connectEngine, getSettings, putSettings } from "../engine";
import {
  applyWhitelistMode,
  branchingToText,
  defaultSettings,
  linesToList,
  listToLines,
  whitelistMode,
  type WhitelistMode,
} from "../settings";
import type { AppSettings } from "../types";

const emit = defineEmits<{
  back: [];
  setup: [];
}>();

const settings = ref<AppSettings>(defaultSettings());
const mainText = ref("");
const globalText = ref("");
const branchingText = ref("");
const mode = ref<WhitelistMode>("none");
const error = ref<string | null>(null);
const saved = ref(false);
const busy = ref(false);

function bindForm(next: AppSettings) {
  settings.value = next;
  mainText.value = listToLines(next.policy.whitelist.main);
  globalText.value = listToLines(next.policy.whitelist.global_subcategories);
  branchingText.value = branchingToText(next.policy.whitelist.branching);
  mode.value = whitelistMode(next.policy.whitelist);
}

async function load() {
  busy.value = true;
  error.value = null;
  try {
    await connectEngine();
    bindForm(await getSettings());
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
    const next = {
      ...settings.value,
      policy: {
        ...settings.value.policy,
        whitelist: {
          ...applyWhitelistMode(
            settings.value.policy.whitelist,
            mode.value,
            globalText.value,
            branchingText.value,
          ),
          main: linesToList(mainText.value),
        },
      },
    };
    bindForm(await putSettings(next));
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
  <section class="settings page" aria-labelledby="settings-title">
    <nav class="crumb" aria-label="Breadcrumb">
      <button type="button" class="crumb-link" @click="emit('back')">Workspace</button>
      <span aria-hidden="true">/</span>
      <span aria-current="page">Settings</span>
    </nav>
    <header class="settings-head">
      <div>
        <h1 id="settings-title">Settings</h1>
        <p class="muted">
          This page is for the Custom intent. Other intents keep their presets. Nothing is moved
          until you Apply on the workspace.
        </p>
      </div>
      <div class="row">
        <button type="button" @click="emit('setup')">
          <Icon :path="mdiDownloadOutline" :size="18" />
          Open Setup
        </button>
        <button type="button" :disabled="busy" @click="save">
          <Icon :path="mdiContentSaveOutline" :size="18" />
          Save
        </button>
        <button type="button" class="primary" @click="emit('back')">
          <Icon :path="mdiArrowLeft" :size="18" />
          Back to workspace
        </button>
      </div>
    </header>
    <p v-if="error" class="error">{{ error }}</p>
    <p v-else-if="saved" class="muted">Saved. Scan with Custom to use them.</p>

    <article class="card">
      <h2>Scan</h2>
      <label class="choice">
        <input v-model="settings.scan.include_hidden" type="checkbox" />
        Include hidden files
      </label>
      <label class="choice">
        <input v-model="settings.scan.protect_projects" type="checkbox" />
        Protect detected projects
      </label>
      <label class="choice">
        <input v-model="settings.scan.extract_metadata" type="checkbox" />
        Extract media and document metadata
      </label>
      <label class="field">
        Max depth (`0` = unlimited)
        <input v-model.number="settings.scan.max_depth" type="number" min="0" />
      </label>
    </article>

    <article class="card">
      <h2>Classification</h2>
      <label class="field">
        Folder style
        <select v-model="settings.policy.style">
          <option value="consistent">Consistent (Documents, Pictures, Music)</option>
          <option value="refined">Refined (Podcasts, Screenshots when evidence exists)</option>
        </select>
      </label>
      <label class="choice">
        <input v-model="settings.policy.use_subfolders" type="checkbox" />
        Use artist/topic subfolders
      </label>
      <label class="field">
        Category language
        <input v-model="settings.policy.category_language" placeholder="en" />
        <span class="muted">Canonical English internally until translations exist.</span>
      </label>
    </article>

    <article class="card">
      <h2>Analysis</h2>
      <p class="muted">
        Models themselves are chosen on the Setup page. Enabling a slot here records the intent.
        Scan runs that analysis when the slot is assigned and the LLM worker is installed.
      </p>
      <label class="choice">
        <input v-model="settings.analyze_images" type="checkbox" />
        Analyze images
      </label>
      <label class="choice">
        <input v-model="settings.analyze_documents" type="checkbox" />
        Analyze documents
      </label>
      <label class="choice">
        <input v-model="settings.policy.rename_media" type="checkbox" />
        Rename media from tags
      </label>
      <label class="choice">
        <input v-model="settings.policy.rename_images_with_date" type="checkbox" />
        Prefix images with capture date
      </label>
    </article>

    <WhitelistEditor
      :mode="mode"
      :main-text="mainText"
      :global-text="globalText"
      :branching-text="branchingText"
      @update:mode="mode = $event"
      @update:main-text="mainText = $event"
      @update:global-text="globalText = $event"
      @update:branching-text="branchingText = $event"
    />
  </section>
</template>
