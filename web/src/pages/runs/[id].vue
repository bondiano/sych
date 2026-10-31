<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'

import { formatMicroUsd } from '@/common/utilities/formatMicroUsd'
import { formatTime } from '@/common/utilities/formatTime'
import { AttemptsTable, ResultSummary, useAttempts, useResult, useRun } from '@/modules/Runs'
import { ErrorNotice, Skeleton, StatusBadge } from '@/modules/Ui'

const route = useRoute()
const id = computed(() => String(route.params.id))
const run = useRun(id)
const result = useResult(id)
const attempts = useAttempts(id)
</script>

<template>
  <ErrorNotice v-if="run.error.value" :error="run.error.value" />
  <Skeleton v-else-if="!run.data.value" class="h-40 w-full" />
  <div v-else class="space-y-8">
    <header class="space-y-2">
      <h1 class="text-xl font-semibold">Run, revision {{ run.data.value.generation }}</h1>
      <div class="flex flex-wrap items-center gap-2 text-sm">
        <StatusBadge :value="run.data.value.state" />
        <StatusBadge :value="run.data.value.coverage" />
        <span class="text-muted-foreground">
          {{ formatTime(run.data.value.created_at) }} ·
          {{ run.data.value.usage.model_attempts }} model calls ·
          {{ formatMicroUsd(run.data.value.usage.cost_known_micro_usd) }} known
          <template v-if="run.data.value.usage.unknown_cost_attempts">
            + {{ run.data.value.usage.unknown_cost_attempts }} of unknown cost
          </template>
        </span>
        <RouterLink :to="`/changes/${run.data.value.change_id}`" class="hover:underline">
          Pull request
        </RouterLink>
        <RouterLink :to="`/runs/${id}/tour`" class="hover:underline">Tour</RouterLink>
      </div>
      <p v-if="run.data.value.failure" class="text-sm text-destructive">
        {{ run.data.value.failure }}
      </p>
    </header>
    <section>
      <h2 class="mb-3 text-lg font-medium">Result</h2>
      <ErrorNotice v-if="result.error.value" :error="result.error.value" />
      <ResultSummary v-else-if="result.data.value" :result="result.data.value" />
      <p v-else-if="result.data.value === null" class="text-sm text-muted-foreground">
        No result to show: the run has not committed one, or retention deleted it.
      </p>
    </section>
    <section>
      <h2 class="mb-3 text-lg font-medium">Model calls</h2>
      <ErrorNotice v-if="attempts.error.value" :error="attempts.error.value" />
      <AttemptsTable v-else-if="attempts.data.value" :attempts="attempts.data.value" />
    </section>
  </div>
</template>
