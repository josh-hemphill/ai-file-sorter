<script setup lang="ts">
import { ref } from "vue";
import SettingsView from "./views/SettingsView.vue";
import SetupView from "./views/SetupView.vue";
import WorkspaceView from "./views/WorkspaceView.vue";
import type { AppSurface } from "./types";

const surface = ref<AppSurface>("workspace");
</script>

<template>
  <div class="app-root">
    <header class="app-bar">
      <strong>AI File Sorter</strong>
      <nav class="app-nav" aria-label="App surfaces">
        <button
          type="button"
          :class="{ active: surface === 'workspace' }"
          @click="surface = 'workspace'"
        >
          Workspace
        </button>
        <button
          type="button"
          :class="{ active: surface === 'settings' }"
          @click="surface = 'settings'"
        >
          Settings
        </button>
        <button
          type="button"
          :class="{ active: surface === 'setup' }"
          @click="surface = 'setup'"
        >
          Setup
        </button>
      </nav>
    </header>
    <WorkspaceView
      v-show="surface === 'workspace'"
      @open-settings="surface = 'settings'"
    />
    <SettingsView
      v-if="surface === 'settings'"
      @back="surface = 'workspace'"
      @setup="surface = 'setup'"
    />
    <SetupView v-if="surface === 'setup'" @back="surface = 'workspace'" />
  </div>
</template>
