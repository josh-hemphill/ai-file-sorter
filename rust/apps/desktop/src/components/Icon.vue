<script setup lang="ts">
withDefaults(
  defineProps<{
    path: string;
    size?: number;
    label?: string;
  }>(),
  { size: 20 },
);
</script>

<template>
  <svg
    xmlns="http://www.w3.org/2000/svg"
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    :aria-hidden="label ? undefined : true"
    :aria-label="label"
    :role="label ? 'img' : 'presentation'"
    class="icon"
  >
    <path :d="path" fill="currentColor" />
  </svg>
</template>
