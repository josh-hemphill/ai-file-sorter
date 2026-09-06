<script setup lang="ts">
import { mdiCogOutline, mdiDownloadOutline, mdiFolderHomeOutline } from "@mdi/js";
import { ref } from "vue";
import Icon from "./components/Icon.vue";
import SettingsView from "./views/SettingsView.vue";
import SetupView from "./views/SetupView.vue";
import WorkspaceView from "./views/WorkspaceView.vue";
import type { AppSurface } from "./types";

const surface = ref<AppSurface>("workspace");

const pages = [
  { id: "workspace" as const, label: "Workspace", icon: mdiFolderHomeOutline },
  { id: "settings" as const, label: "Settings", icon: mdiCogOutline },
  { id: "setup" as const, label: "Setup", icon: mdiDownloadOutline },
];
</script>

<template>
  <div class="app-root">
    <header class="app-bar">
      <strong>AI File Sorter</strong>
      <nav class="app-nav" aria-label="Pages">
        <button
          v-for="page in pages"
          :key="page.id"
          type="button"
          class="app-nav-link"
          :class="{ active: surface === page.id }"
          :aria-current="surface === page.id ? 'page' : undefined"
          @click="surface = page.id"
        >
          <Icon :path="page.icon" :size="18" />
          {{ page.label }}
        </button>
      </nav>
    </header>
    <WorkspaceView
      v-show="surface === 'workspace'"
      :active="surface === 'workspace'"
      @open-settings="surface = 'settings'"
      @open-setup="surface = 'setup'"
    />
    <SettingsView
      v-if="surface === 'settings'"
      @back="surface = 'workspace'"
      @setup="surface = 'setup'"
    />
    <SetupView v-if="surface === 'setup'" @back="surface = 'workspace'" />
  </div>
</template>
