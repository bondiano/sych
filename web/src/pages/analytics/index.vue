<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import { toId } from '@/common/utilities/toId'
import { QualityView, useQuality } from '@/modules/Analytics'
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
    </p>
    <ErrorNotice v-if="quality.error.value" :error="quality.error.value" />
    <Skeleton v-else-if="!quality.data.value" class="h-40 w-full" />
    <QualityView v-else :quality="quality.data.value" />
  </div>
</template>
