<script setup lang="ts">
import { CircleAlert, CircleCheck, TriangleAlert } from '@lucide/vue'
import { computed } from 'vue'

import type { Schemas } from '@/modules/Api'
import { Alert, AlertDescription, AlertTitle } from '@/modules/Ui'

import { strings, type Lang } from '../i18n'
import { verdictFromFindings } from '../report'

const props = defineProps<{
  result?: Schemas['ReviewResult']
  view?: Schemas['ReportView']
  language?: Lang
}>()

const SHOWN = {
  ok: {
    icon: CircleCheck,
    classes:
      'border-emerald-200 bg-emerald-50 text-emerald-950 dark:border-emerald-900 dark:bg-emerald-950 dark:text-emerald-100',
  },
  warn: {
    icon: TriangleAlert,
    classes:
      'border-amber-300/80 bg-amber-50 text-amber-950 dark:border-amber-800/70 dark:bg-amber-950/70 dark:text-amber-100',
  },
  bad: {
    icon: CircleAlert,
    classes:
      'border-red-200 bg-red-50 text-red-950 dark:border-red-900 dark:bg-red-950 dark:text-red-100',
  },
}

const lang = computed<Lang>(
  () => props.language ?? props.view?.header.language ?? props.result?.language ?? 'en',
)
const t = computed(() => strings(lang.value))
const shown = computed(() => {
  if (props.view) {
    const openOrAll = props.view.findings.filter((f) => f.lifecycle === 'OPEN')
    return verdictFromFindings(openOrAll, props.view.header.coverage, lang.value)
  }

  if (props.result) {
    return verdictFromFindings(props.result.findings, props.result.coverage, lang.value)
  }

  return verdictFromFindings([], 'UNAVAILABLE', lang.value)
})
const limitReached = computed(() => {
  const outcome = props.view?.header.execution_outcome ?? props.result?.execution_outcome
  return outcome === 'LIMIT_REACHED'
})
</script>

<template>
  <Alert :class="['px-4 py-3', SHOWN[shown.tone].classes]">
    <component :is="SHOWN[shown.tone].icon" aria-hidden="true" />
    <AlertTitle class="text-base font-semibold">{{ shown.title }}</AlertTitle>
    <AlertDescription class="text-current/85">
      <p>
        {{ shown.coverage }}
        <template v-if="limitReached">
          {{ t.limitReached }}
        </template>
      </p>
    </AlertDescription>
  </Alert>
</template>
