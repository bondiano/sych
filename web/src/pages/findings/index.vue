<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import { csvHref } from '@/common/utilities/csvHref'
import { toId } from '@/common/utilities/toId'
import { FindingFilters, FindingsTable, useFindings } from '@/modules/Findings'
import { RepositorySelect } from '@/modules/Session'
import { ErrorNotice, LoadMore } from '@/modules/Ui'

const repository = useQueryParam('repository')
const state = useQueryParam('state')
const severity = useQueryParam('severity')
const filter = computed(() => ({
  repository_id: toId(repository.value),
  state: state.value,
  severity: severity.value,
}))
const findings = useFindings(filter)
const rows = computed(() => findings.data.value?.pages.flatMap((p) => p.items) ?? [])
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="mr-auto text-xl font-semibold">Findings</h1>
      <RepositorySelect v-model="repository" />
      <FindingFilters v-model:state="state" v-model:severity="severity" />
      <a
        :href="csvHref('/v1/findings', filter)"
        class="text-sm underline"
        title="Up to 10 000 findings, newest first"
        download
        >CSV</a
      >
    </div>
    <ErrorNotice v-if="findings.error.value" :error="findings.error.value" />
    <FindingsTable :findings="rows" show-change />
    <LoadMore
      :more="findings.hasNextPage.value"
      :loading="findings.isFetchingNextPage.value"
      @next="findings.fetchNextPage()"
    />
  </div>
</template>
