<script setup lang="ts">
import { WORKFLOW_STEPS } from "../workflow";
import type { WorkflowStep } from "../types";

const props = defineProps<{
  current: WorkflowStep;
}>();

function stepState(id: WorkflowStep): "done" | "current" | "upcoming" {
  const order = WORKFLOW_STEPS.map((step) => step.id);
  const here = order.indexOf(props.current);
  const index = order.indexOf(id);
  if (index < here) {
    return "done";
  }
  if (index === here) {
    return "current";
  }
  return "upcoming";
}
</script>

<template>
  <ol class="stepper" aria-label="Organization steps">
    <li
      v-for="step in WORKFLOW_STEPS"
      :key="step.id"
      :class="stepState(step.id)"
      :aria-current="step.id === current ? 'step' : undefined"
    >
      {{ step.label }}
    </li>
  </ol>
</template>
