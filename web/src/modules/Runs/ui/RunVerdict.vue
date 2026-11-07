<script setup lang="ts">
import { CircleAlert, CircleCheck, TriangleAlert } from '@lucide/vue'
import { computed } from 'vue'

import type { Schemas } from '@/modules/Api'
import { Alert, AlertDescription, AlertTitle } from '@/modules/Ui'

import { verdict } from '../report'

const props = defineProps<{ result: Schemas['ReviewResult'] }>()

const SHOWN = {
  ok: {
    icon: CircleCheck,
    classes:
      'border-emerald-200 bg-emerald-50 text-emerald-950 dark:border-emerald-900 dark:bg-emerald-950 dark:text-emerald-100',
  },
  warn: {
    icon: TriangleAlert,
    classes:
      'border-amber-200 bg-amber-50 text-amber-950 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-100',
  },
  bad: {
    icon: CircleAlert,
    classes:
      'border-red-200 bg-red-50 text-red-950 dark:border-red-900 dark:bg-red-950 dark:text-red-100',
  },
}

const shown = computed(() => verdict(props.result))
</script>

<template>
  <Alert :class="['px-4 py-3', SHOWN[shown.tone].classes]">
    <component :is="SHOWN[shown.tone].icon" aria-hidden="true" />
    <AlertTitle class="text-base font-semibold">{{ shown.title }}</AlertTitle>
    <AlertDescription class="text-current/85">
      <p>
        {{ shown.coverage }}
        <template v-if="result.execution_outcome === 'LIMIT_REACHED'">
          A budget or size limit stopped part of the review before it finished.
        </template>
      </p>
    </AlertDescription>
  </Alert>
</template>
