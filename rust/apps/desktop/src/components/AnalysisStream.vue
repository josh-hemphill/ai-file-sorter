<script setup lang="ts">
import type { LogEvent, ProgressEvent } from "../types";

defineProps<{
  stages: { id: string; label: string; current: number; total: number | null; message: string }[];
  lines: LogEvent[];
  progress: ProgressEvent | null;
}>();
</script>

<template>
  <div class="analysis">
    <table class="stages" v-if="stages.length">
      <thead>
        <tr>
          <th>Stage</th>
          <th>Progress</th>
          <th>Current</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="stage in stages" :key="stage.id">
          <td>{{ stage.label }}</td>
          <td>
            {{ stage.current }}<template v-if="stage.total">/{{ stage.total }}</template>
          </td>
          <td class="muted">{{ stage.message }}</td>
        </tr>
      </tbody>
    </table>
    <p v-else-if="progress" class="muted">
      {{ progress.stage }} {{ progress.current
      }}<template v-if="progress.total">/{{ progress.total }}</template>
      — {{ progress.message }}
    </p>
    <ol class="stream" aria-label="Analysis stream" aria-live="polite">
      <li v-for="(line, index) in lines" :key="index" :class="line.level">
        {{ line.message }}
      </li>
    </ol>
    <p v-if="!lines.length" class="muted">
      File identifications appear here while a scan runs so you can catch problems early.
    </p>
  </div>
</template>
