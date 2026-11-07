<script setup lang="ts">
import { computed } from 'vue'

import { proseParts } from '../report'

const props = defineProps<{ text: string }>()
const parts = computed(() => proseParts(props.text))
</script>

<template>
  <template v-for="(part, i) in parts" :key="i">
    <code v-if="part.code" class="rounded bg-muted px-1 py-px font-mono text-[0.85em]">{{
      part.text
    }}</code>
    <template v-else>{{ part.text }}</template>
  </template>
</template>
