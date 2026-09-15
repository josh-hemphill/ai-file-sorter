<script setup lang="ts">
import { mdiChevronDown, mdiChevronRight, mdiFileOutline, mdiFolderOutline } from "@mdi/js";
import { ref } from "vue";
import type { SourceTreeNode } from "../tree";
import Icon from "./Icon.vue";

defineOptions({ name: "SourceTree" });

const props = withDefaults(
  defineProps<{
    node: SourceTreeNode;
    selectedAsset?: string | null;
  }>(),
  { selectedAsset: null },
);

const emit = defineEmits<{
  select: [assetId: string];
}>();

const open = ref(true);

function toggle() {
  if (props.node.kind !== "directory") {
    return;
  }
  open.value = !open.value;
}

function onActivate() {
  if (props.node.kind === "file" && props.node.assetId) {
    emit("select", props.node.assetId);
    return;
  }
  toggle();
}

function onSelect(assetId: string) {
  emit("select", assetId);
}
</script>

<template>
  <li
    class="source-item"
    role="treeitem"
    :aria-expanded="node.kind === 'directory' ? open : undefined"
    :aria-selected="node.kind === 'file' && selectedAsset === node.assetId ? true : undefined"
  >
    <button
      type="button"
      class="source-row"
      :class="{ selected: node.kind === 'file' && selectedAsset === node.assetId }"
      :aria-label="node.kind === 'directory' ? `${node.name} folder` : node.name"
      @click="onActivate"
    >
      <span class="source-twist" aria-hidden="true">
        <Icon
          v-if="node.kind === 'directory'"
          :path="open ? mdiChevronDown : mdiChevronRight"
          :size="16"
        />
      </span>
      <Icon
        :path="node.kind === 'directory' ? mdiFolderOutline : mdiFileOutline"
        :size="16"
        aria-hidden="true"
      />
      <span class="source-name">{{ node.name }}</span>
      <span
        v-for="chip in node.chips"
        :key="chip.kind"
        class="chip"
        :class="`chip-${chip.kind}`"
        :title="chip.title"
      >
        {{ chip.label }}
      </span>
      <span v-if="node.kind === 'directory'" class="source-count">· {{ node.fileCount }}</span>
    </button>
    <ul v-if="node.kind === 'directory' && open && node.children.length" class="source-tree" role="group">
      <SourceTree
        v-for="child in node.children"
        :key="child.path"
        :node="child"
        :selected-asset="selectedAsset"
        @select="onSelect"
      />
    </ul>
  </li>
</template>
