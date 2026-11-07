<script setup lang="ts">
import type { Schemas } from '@/modules/Api'
import { StatusBadge } from '@/modules/Ui'

import RunChecks from './RunChecks.vue'

defineProps<{ result: Schemas['ReviewResult'] }>()
</script>

<template>
  <div class="space-y-6">
    <div class="flex flex-wrap items-center gap-2 text-sm">
      <StatusBadge :value="result.recommendation" />
      <StatusBadge :value="result.coverage" />
      <StatusBadge :value="result.execution_outcome" />
      <span class="text-muted-foreground">revision {{ result.review_revision }}</span>
    </div>
    <RunChecks :result="result" />
  </div>
</template>
