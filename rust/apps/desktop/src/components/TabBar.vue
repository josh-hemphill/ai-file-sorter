<script setup lang="ts">
import { CENTER_TABS } from "../workflow";
import type { CenterView } from "../types";

defineProps<{
  view: CenterView;
  counts?: Partial<Record<CenterView, number>>;
}>();

const emit = defineEmits<{
  select: [view: CenterView];
}>();
</script>

<template>
  <div class="tab-bar" role="tablist" aria-label="Workspace views">
    <button
      v-for="tab in CENTER_TABS"
      :id="`tab-${tab.id}`"
      :key="tab.id"
      type="button"
      role="tab"
      class="tab"
      :aria-selected="view === tab.id"
      :aria-controls="`panel-${tab.id}`"
      :tabindex="view === tab.id ? 0 : -1"
      :class="{ active: view === tab.id }"
      @click="emit('select', tab.id)"
    >
      {{ tab.label }}
      <span v-if="counts?.[tab.id] != null" class="tab-count">{{ counts[tab.id] }}</span>
    </button>
  </div>
</template>
