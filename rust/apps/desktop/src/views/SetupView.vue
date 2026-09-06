<script setup lang="ts">
const emit = defineEmits<{
  back: [];
}>();

const slots = [
  {
    id: "categorize",
    label: "Categorization",
    hint: "Folder and subcategory labels from a text model.",
  },
  {
    id: "vision",
    label: "Image description",
    hint: "Visual content plus EXIF context.",
  },
  {
    id: "document",
    label: "Document analysis",
    hint: "PDF and Office text for topics and rename suggestions.",
  },
  {
    id: "chat",
    label: "Assistant chat",
    hint: "Natural-language patches. Heuristic tools still work when this is off.",
  },
];
</script>

<template>
  <section class="settings">
    <header class="settings-head">
      <div>
        <h1>Setup</h1>
        <p class="muted">
          Each analysis slot can be off, a downloaded local model, or a remote/custom endpoint.
          The workspace stays usable with every slot off.
        </p>
      </div>
      <button type="button" class="primary" @click="emit('back')">Back to workspace</button>
    </header>
    <article v-for="slot in slots" :key="slot.id" class="card">
      <strong>{{ slot.label }}</strong>
      <span class="muted">{{ slot.hint }}</span>
      <p class="badge">Off</p>
    </article>
  </section>
</template>
