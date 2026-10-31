<script setup lang="ts">
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/modules/Ui'

const state = defineModel<string>('state')
const severity = defineModel<string>('severity')

// An empty choice clears the filter; a select item cannot carry an empty value.
const ANY = 'any'
const STATES = ['OPEN', 'RESOLVED', 'DISMISSED', 'OBSOLETE']
const SEVERITIES = ['CRITICAL', 'HIGH', 'MEDIUM', 'LOW', 'INFO']
const pick = (value: unknown) => (value === ANY || typeof value !== 'string' ? undefined : value)
</script>

<template>
  <div class="flex flex-wrap gap-3">
    <Select :model-value="state ?? ANY" @update:model-value="state = pick($event)">
      <SelectTrigger class="w-40" aria-label="State"><SelectValue /></SelectTrigger>
      <SelectContent>
        <SelectItem :value="ANY">Any state</SelectItem>
        <SelectItem v-for="s in STATES" :key="s" :value="s">{{ s.toLowerCase() }}</SelectItem>
      </SelectContent>
    </Select>
    <Select :model-value="severity ?? ANY" @update:model-value="severity = pick($event)">
      <SelectTrigger class="w-40" aria-label="Severity"><SelectValue /></SelectTrigger>
      <SelectContent>
        <SelectItem :value="ANY">Any severity</SelectItem>
        <SelectItem v-for="s in SEVERITIES" :key="s" :value="s">{{ s.toLowerCase() }}</SelectItem>
      </SelectContent>
    </Select>
  </div>
</template>
