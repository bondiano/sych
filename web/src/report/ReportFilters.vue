<script setup lang="ts">
import { Search, X } from '@lucide/vue'
import { computed } from 'vue'

import type { Schemas } from '@/modules/Api'
import { categoryLabel, strings, type Lang } from '@/modules/Runs'
import { Button, statusLabel } from '@/modules/Ui'

const SEVERITIES: Schemas['Severity'][] = ['CRITICAL', 'HIGH', 'MEDIUM', 'LOW', 'INFO']
const CATEGORIES: Schemas['FindingCategory'][] = ['DEFECT', 'RULE_VIOLATION', 'IMPROVEMENT', 'NIT']

const props = defineProps<{
  severity: Schemas['Severity'] | ''
  category: Schemas['FindingCategory'] | ''
  fileQuery: string
  language?: Lang
}>()

const emit = defineEmits<{
  'update:severity': [value: Schemas['Severity'] | '']
  'update:category': [value: Schemas['FindingCategory'] | '']
  'update:fileQuery': [value: string]
  reset: []
}>()

const t = computed(() => strings(props.language))
const hasActive = computed(
  () => props.severity !== '' || props.category !== '' || props.fileQuery.trim() !== '',
)

function onSeverity(event: Event) {
  const value = (event.target as HTMLSelectElement).value as Schemas['Severity'] | ''
  emit('update:severity', value)
}

function onCategory(event: Event) {
  const value = (event.target as HTMLSelectElement).value as Schemas['FindingCategory'] | ''
  emit('update:category', value)
}

function onFile(event: Event) {
  emit('update:fileQuery', (event.target as HTMLInputElement).value)
}
</script>

<template>
  <div class="flex flex-wrap items-center gap-3 print:hidden">
    <label class="flex items-center gap-2 text-xs text-muted-foreground">
      <span class="sr-only">{{ t.filterSeverity }}</span>
      <select
        :value="severity"
        :aria-label="t.filterSeverity"
        class="h-8 rounded-md border bg-card px-2.5 text-xs text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        @change="onSeverity"
      >
        <option value="">{{ t.allSeverities }}</option>
        <option v-for="s in SEVERITIES" :key="s" :value="s">{{ statusLabel(s) }}</option>
      </select>
    </label>

    <label class="flex items-center gap-2 text-xs text-muted-foreground">
      <span class="sr-only">{{ t.filterCategory }}</span>
      <select
        :value="category"
        :aria-label="t.filterCategory"
        class="h-8 rounded-md border bg-card px-2.5 text-xs text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        @change="onCategory"
      >
        <option value="">{{ t.allCategories }}</option>
        <option v-for="c in CATEGORIES" :key="c" :value="c">
          {{ categoryLabel(c, language) }}
        </option>
      </select>
    </label>

    <div class="relative min-w-52 flex-1 sm:max-w-xs">
      <Search
        class="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground"
        aria-hidden="true"
      />
      <input
        type="search"
        :value="fileQuery"
        :placeholder="t.filterFilePlaceholder"
        :aria-label="t.filterFilePlaceholder"
        class="h-8 w-full rounded-md border bg-card pr-3 pl-8 font-mono text-xs text-foreground placeholder:font-sans placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        @input="onFile"
      />
    </div>

    <Button v-if="hasActive" variant="ghost" size="sm" type="button" @click="emit('reset')">
      <X class="size-3.5" aria-hidden="true" />
      {{ t.clearFilters }}
    </Button>
  </div>
</template>
