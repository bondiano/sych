<script setup lang="ts">
import { computed } from 'vue'

import { formatMicroUsd } from '@/common/utilities/formatMicroUsd'
import type { Schemas } from '@/modules/Api'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/modules/Ui'

import { everyDay } from '../days'

const props = defineProps<{ usage: Schemas['Usage'] }>()

const days = computed(() => everyDay(props.usage))
const peak = computed(() => Math.max(1, ...days.value.map((d) => d.cost_known_micro_usd)))
const unknown = computed(() => props.usage.days.reduce((n, d) => n + d.unknown_cost_attempts, 0))
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>Model cost by day</CardTitle>
      <CardDescription>
        Known charges per UTC day, {{ usage.from }} to {{ usage.to }}.
        <template v-if="unknown">
          {{ unknown }} calls of unknown cost are not in the bars.</template
        >
      </CardDescription>
    </CardHeader>
    <CardContent>
      <p v-if="!usage.days.length" class="text-sm text-muted-foreground">No model calls.</p>
      <ul v-else class="flex h-40 items-end gap-1" aria-label="Known cost per day">
        <li
          v-for="d in days"
          :key="d.day"
          class="min-h-px flex-1 rounded-t bg-primary"
          :style="{ height: `${(d.cost_known_micro_usd / peak) * 100}%` }"
          :title="`${d.day}: ${formatMicroUsd(d.cost_known_micro_usd)}, ${d.attempts} calls${d.unknown_cost_attempts ? `, ${d.unknown_cost_attempts} of unknown cost` : ''}`"
        />
      </ul>
      <p
        v-for="f in usage.subscription_fees"
        :key="`${f.provider}-${f.month}`"
        class="mt-3 text-sm"
      >
        Subscription {{ f.provider }}, {{ f.month }}: {{ formatMicroUsd(f.amount_micro_usd) }}
      </p>
    </CardContent>
  </Card>
</template>
