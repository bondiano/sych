<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import { toId } from '@/common/utilities/toId'
import { CostByDay, SummaryTiles, useSummary, useUsage } from '@/modules/Overview'
import { RepositorySelect } from '@/modules/Session'
import {
  ErrorNotice,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
} from '@/modules/Ui'

const repository = useQueryParam('repository')
const period = useQueryParam('days')
const repositoryId = computed(() => toId(repository.value))
const days = computed(() => Number(period.value ?? 30))
const summary = useSummary(repositoryId, days)
const usage = useUsage(repositoryId, days)
</script>

<template>
  <div class="space-y-6">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="mr-auto text-xl font-semibold">Overview</h1>
      <RepositorySelect v-model="repository" />
      <Select :model-value="String(days)" @update:model-value="period = String($event)">
        <SelectTrigger class="w-36" aria-label="Period"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem v-for="d in [7, 30, 90]" :key="d" :value="String(d)"
            >Last {{ d }} days</SelectItem
          >
        </SelectContent>
      </Select>
    </div>
    <ErrorNotice v-if="summary.error.value" :error="summary.error.value" />
    <Skeleton v-else-if="!summary.data.value" class="h-40 w-full" />
    <SummaryTiles v-else :summary="summary.data.value" />
    <CostByDay v-if="usage.data.value" :usage="usage.data.value" />
  </div>
</template>
