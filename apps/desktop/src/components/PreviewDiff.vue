<script setup lang="ts">
import type { PlanCounts, PreviewRow } from "../workflow";

defineProps<{
  rows: PreviewRow[];
  counts: PlanCounts;
  journalStatus?: string | null;
  dryRun?: boolean;
}>();
</script>

<template>
  <section class="preview-diff" aria-label="Planned changes">
    <p class="preview-summary">
      {{ counts.moves }} moves · {{ counts.creates }} folders created ·
      {{ counts.removes }} empty folders removed
      <template v-if="journalStatus">
        · {{ dryRun ? "dry run" : journalStatus }}
      </template>
    </p>
    <table v-if="rows.length">
      <thead>
        <tr>
          <th>Change</th>
          <th>From</th>
          <th>To</th>
          <th>State</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="row in rows" :key="row.seq">
          <td>{{ row.kind }}</td>
          <td>{{ row.from ?? "—" }}</td>
          <td>{{ row.to ?? row.label }}</td>
          <td>
            {{ row.state ?? "planned" }}
            <span v-if="row.detail" class="muted"> · {{ row.detail }}</span>
          </td>
        </tr>
      </tbody>
    </table>
    <p v-else class="muted">Validate accepted changes to see the from → to preview.</p>
  </section>
</template>
