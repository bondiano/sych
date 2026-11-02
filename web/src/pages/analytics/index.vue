<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import { toId } from '@/common/utilities/toId'
import {
  CostView,
  HonestyView,
  LatencyView,
  QualityView,
  useCost,
  useHonesty,
  useLatency,
  useQuality,
} from '@/modules/Analytics'
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
const days = computed(() => toId(period.value) ?? 30)
const quality = useQuality(repositoryId, days)
const honesty = useHonesty(repositoryId, days)
const cost = useCost(repositoryId, days)
const latency = useLatency(repositoryId, days)
</script>

<template>
  <div class="space-y-6">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="mr-auto text-xl font-semibold">Analytics</h1>
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
    <p class="text-sm text-muted-foreground">
      A share needs five outcomes; below that only the counts are shown.
      <a
        :href="`/v1/reports/weekly?format=markdown${repositoryId ? `&repository_id=${repositoryId}` : ''}`"
        class="underline"
        >Weekly report as Markdown</a
      >.
    </p>
    <h2 class="text-lg font-semibold">Usefulness</h2>
    <ErrorNotice v-if="quality.error.value" :error="quality.error.value" />
    <Skeleton v-else-if="!quality.data.value" class="h-40 w-full" />
    <QualityView v-else :quality="quality.data.value" />
    <h2 class="text-lg font-semibold">Honesty</h2>
    <ErrorNotice v-if="honesty.error.value" :error="honesty.error.value" />
    <Skeleton v-else-if="!honesty.data.value" class="h-40 w-full" />
    <HonestyView v-else :honesty="honesty.data.value" />
    <h2 class="text-lg font-semibold">Cost</h2>
    <ErrorNotice v-if="cost.error.value" :error="cost.error.value" />
    <Skeleton v-else-if="!cost.data.value" class="h-40 w-full" />
    <CostView v-else :cost="cost.data.value" />
    <h2 class="text-lg font-semibold">Latency</h2>
    <ErrorNotice v-if="latency.error.value" :error="latency.error.value" />
    <Skeleton v-else-if="!latency.data.value" class="h-24 w-full" />
    <LatencyView v-else :latency="latency.data.value" />
  </div>
</template>
