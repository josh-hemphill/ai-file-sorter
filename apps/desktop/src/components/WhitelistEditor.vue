<script setup lang="ts">
import type { WhitelistMode } from "../settings";

defineProps<{
  mode: WhitelistMode;
  mainText: string;
  globalText: string;
  branchingText: string;
}>();

const emit = defineEmits<{
  "update:mode": [mode: WhitelistMode];
  "update:mainText": [value: string];
  "update:globalText": [value: string];
  "update:branchingText": [value: string];
}>();
</script>

<template>
  <article class="card">
    <h2>Category whitelist</h2>
    <p class="muted">
      Empty lists keep the current heuristic names. Global subcategories and
      per-category branching cannot be used together.
    </p>
    <label class="field">
      Main categories (one per line)
      <textarea
        :value="mainText"
        rows="5"
        @input="emit('update:mainText', ($event.target as HTMLTextAreaElement).value)"
      />
    </label>
    <p class="label">Second-level folders</p>
    <label class="choice">
      <input
        type="radio"
        name="whitelist-mode"
        :checked="mode === 'none'"
        @change="emit('update:mode', 'none')"
      />
      Any heuristic subfolder
    </label>
    <label class="choice">
      <input
        type="radio"
        name="whitelist-mode"
        :checked="mode === 'global'"
        @change="emit('update:mode', 'global')"
      />
      Global subcategories
    </label>
    <label class="choice">
      <input
        type="radio"
        name="whitelist-mode"
        :checked="mode === 'branching'"
        @change="emit('update:mode', 'branching')"
      />
      Per-category branching
    </label>
    <label v-if="mode === 'global'" class="field">
      Global subcategories (one per line)
      <textarea
        :value="globalText"
        rows="4"
        @input="emit('update:globalText', ($event.target as HTMLTextAreaElement).value)"
      />
    </label>
    <label v-if="mode === 'branching'" class="field">
      Branching (`Documents: Reports, Notes`)
      <textarea
        :value="branchingText"
        rows="4"
        @input="emit('update:branchingText', ($event.target as HTMLTextAreaElement).value)"
      />
    </label>
  </article>
</template>
