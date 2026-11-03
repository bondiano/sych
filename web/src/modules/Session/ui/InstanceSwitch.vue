<script setup lang="ts">
import { computed } from 'vue'

import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/modules/Ui'

import { currentInstance, dashboardOf } from '../instances'
import { useMe } from '../session'

const { data: me } = useMe()
const instances = computed(() => me.value?.instances ?? [])
const current = computed(() => currentInstance(instances.value))

// Another server is a full-page move to its own dashboard and sign-in.
function go(name: unknown) {
  const target = instances.value.find((i) => i.name === name && !i.current)
  if (target) window.location.assign(dashboardOf(target))
}
</script>

<template>
  <Select v-if="current" :model-value="current.name" @update:model-value="go">
    <SelectTrigger class="w-40" aria-label="Sych server"><SelectValue /></SelectTrigger>
    <SelectContent>
      <SelectItem v-for="i in instances" :key="i.name" :value="i.name">{{ i.name }}</SelectItem>
    </SelectContent>
  </Select>
</template>
