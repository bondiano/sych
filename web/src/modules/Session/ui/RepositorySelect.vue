<script setup lang="ts">
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/modules/Ui'

import { useMe } from '../session'

/** A repository id as text, or nothing for every repository the caller may read. */
const repository = defineModel<string>()
const { data: me } = useMe()

// A select item cannot carry an empty value, so "every" has its own.
const EVERY = 'every'
const pick = (value: unknown) => (value === EVERY || typeof value !== 'string' ? undefined : value)
</script>

<template>
  <Select :model-value="repository ?? EVERY" @update:model-value="repository = pick($event)">
    <SelectTrigger class="w-56" aria-label="Repository"><SelectValue /></SelectTrigger>
    <SelectContent>
      <SelectItem :value="EVERY">Every repository</SelectItem>
      <SelectItem
        v-for="g in me?.repositories ?? []"
        :key="g.repository.id"
        :value="String(g.repository.id)"
      >
        {{ g.repository.full_name }}
      </SelectItem>
    </SelectContent>
  </Select>
</template>
