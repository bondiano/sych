<script setup lang="ts">
import { computed } from 'vue'

import { label, tone } from '../lib/tone'
import { cn } from '../lib/utils'
import { Badge } from './ui/badge'

const props = defineProps<{ value: string | null | undefined }>()

const CLASSES = {
  ok: 'bg-emerald-100 text-emerald-900 dark:bg-emerald-950 dark:text-emerald-200',
  warn: 'bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-200',
  bad: 'bg-red-100 text-red-900 dark:bg-red-950 dark:text-red-200',
  off: 'bg-muted text-muted-foreground',
}
const classes = computed(() => {
  const t = tone(props.value)
  return t ? CLASSES[t] : undefined
})
</script>

<template>
  <Badge variant="secondary" :class="cn('font-normal', classes)">{{ label(value) }}</Badge>
</template>
