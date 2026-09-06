<script setup lang="ts">
import { CENTER_TABS } from "../workflow";
import type { CenterView } from "../types";

const props = defineProps<{
  view: CenterView;
  counts?: Partial<Record<CenterView, number>>;
}>();

const emit = defineEmits<{
  select: [view: CenterView];
}>();

function selectTab(id: CenterView) {
  emit("select", id);
  queueMicrotask(() => {
    document.getElementById(`tab-${id}`)?.focus();
  });
}

function onTabsKey(event: KeyboardEvent) {
  const ids = CENTER_TABS.map((tab) => tab.id);
  const here = ids.indexOf(props.view);
  if (here < 0) {
    return;
  }
  if (event.key === "ArrowRight" || event.key === "ArrowDown") {
    event.preventDefault();
    selectTab(ids[(here + 1) % ids.length] ?? props.view);
  } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
    event.preventDefault();
    selectTab(ids[(here - 1 + ids.length) % ids.length] ?? props.view);
  } else if (event.key === "Home") {
    event.preventDefault();
    selectTab(ids[0] ?? props.view);
  } else if (event.key === "End") {
    event.preventDefault();
    selectTab(ids[ids.length - 1] ?? props.view);
  }
}
</script>

<template>
  <div
    class="tab-bar"
    role="tablist"
    aria-label="Workspace views"
    @keydown="onTabsKey"
  >
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
      @click="selectTab(tab.id)"
    >
      {{ tab.label }}
      <span v-if="counts?.[tab.id] != null" class="tab-count">{{ counts[tab.id] }}</span>
    </button>
  </div>
</template>
