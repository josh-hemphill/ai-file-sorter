<script setup lang="ts">
const emit = defineEmits<{
  back: [];
  setup: [];
}>();
</script>

<template>
  <section class="settings">
    <header class="settings-head">
      <div>
        <h1>Settings</h1>
        <p class="muted">
          Custom intent uses these options. Other intents keep their presets. Nothing is moved
          until you Apply on the workspace.
        </p>
      </div>
      <div class="row">
        <button type="button" @click="emit('setup')">Set up models</button>
        <button type="button" class="primary" @click="emit('back')">Back to workspace</button>
      </div>
    </header>

    <article class="card">
      <h2>Scan</h2>
      <p class="muted">
        Hidden files, recursion depth, and project protection will be editable here.
      </p>
    </article>
    <article class="card">
      <h2>Classification</h2>
      <p class="muted">
        Consistent vs refined folders, category language, and named whitelists will live here.
      </p>
    </article>
    <article class="card">
      <h2>Analysis</h2>
      <p class="muted">
        Image, document, and media-rename toggles will live here. Models themselves are chosen
        in Setup.
      </p>
    </article>
  </section>
</template>
