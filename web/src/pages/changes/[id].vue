<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'

import { ChangeLog, ChangeRuns, useChange } from '@/modules/Changes'
import { FindingsTable, useFindings } from '@/modules/Findings'
import { ResultSummary, useResult } from '@/modules/Runs'
import { ErrorNotice, LoadMore, Skeleton } from '@/modules/Ui'

const route = useRoute()
const id = computed(() => Number(route.params.id))
const change = useChange(id)
const latest = computed(() => change.data.value?.change.latest_run?.run_id ?? '')
const result = useResult(latest)
const findings = useFindings(computed(() => ({ change_id: id.value })))
const rows = computed(() => findings.data.value?.pages.flatMap((p) => p.items) ?? [])
</script>

<template>
  <ErrorNotice v-if="change.error.value" :error="change.error.value" />
  <Skeleton v-else-if="!change.data.value" class="h-40 w-full" />
  <div v-else class="space-y-8">
    <header class="space-y-1">
      <h1 class="text-xl font-semibold">
        {{ change.data.value.change.repository.full_name }}#{{ change.data.value.change.number }}
      </h1>
      <p class="text-sm text-muted-foreground">
        {{ change.data.value.change.scm_state?.toLowerCase() }} · head
        <span class="font-mono">{{ change.data.value.change.head_sha?.slice(0, 12) }}</span>
      </p>
    </header>
    <section v-if="latest">
      <h2 class="mb-3 text-lg font-medium">Latest review</h2>
      <ErrorNotice v-if="result.error.value" :error="result.error.value" />
      <ResultSummary v-else-if="result.data.value" :result="result.data.value" />
      <p v-else-if="result.data.value === null" class="text-sm text-muted-foreground">
        No result to show: the run has not committed one, or retention deleted it.
      </p>
    </section>
    <section>
      <h2 class="mb-3 text-lg font-medium">Findings</h2>
      <FindingsTable :findings="rows" />
      <LoadMore
        :more="findings.hasNextPage.value"
        :loading="findings.isFetchingNextPage.value"
        @next="findings.fetchNextPage()"
      />
    </section>
    <section>
      <h2 class="mb-3 text-lg font-medium">Revisions</h2>
      <ChangeRuns :runs="change.data.value.runs" />
    </section>
    <ChangeLog :change="change.data.value" />
  </div>
</template>
