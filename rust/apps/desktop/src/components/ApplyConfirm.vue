<script setup lang="ts">
defineProps<{
  root: string;
  summary: string;
}>();

const emit = defineEmits<{
  cancel: [];
  confirm: [];
}>();
</script>

<template>
  <div class="modal-backdrop" role="presentation" @click.self="emit('cancel')">
    <div
      class="modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="apply-confirm-title"
    >
      <h2 id="apply-confirm-title">Apply these changes?</h2>
      <p>{{ summary }}</p>
      <p class="muted">
        Undo restores this journal only while files still match the scan identity.
        Edits made after Apply cannot be undone here. Destination folders that
        already existed are left in place.
      </p>
      <p class="muted">Source: {{ root }}</p>
      <div class="row">
        <button type="button" @click="emit('cancel')">Cancel</button>
        <button type="button" class="primary" @click="emit('confirm')">Apply</button>
      </div>
    </div>
  </div>
</template>
