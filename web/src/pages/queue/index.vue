<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import { toId } from '@/common/utilities/toId'
import { JOB_STATES, JobsTable, UnresolvedList, useJobs, useUnresolved } from '@/modules/Operations'
import { canSomewhere, RepositorySelect, useMe } from '@/modules/Session'
import {
  ErrorNotice,
  LoadMore,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
  statusLabel,
} from '@/modules/Ui'

const repository = useQueryParam('repository')
const state = useQueryParam('state')
const { data: me } = useMe()
const writer = computed(() => canSomewhere(me.value, 'writer'))
const repositoryId = computed(() => toId(repository.value))
const unresolved = useUnresolved(repositoryId)
const jobs = useJobs(computed(() => ({ repository_id: repositoryId.value, state: state.value })))
const rows = computed(() => jobs.data.value?.pages.flatMap((p) => p.items) ?? [])

// An empty choice clears the filter; a select item cannot carry an empty value.
const ANY = 'any'
const pick = (value: unknown) => (value === ANY || typeof value !== 'string' ? undefined : value)
</script>

<template>
  <div class="space-y-8">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="mr-auto text-xl font-semibold">Queue</h1>
      <RepositorySelect v-model="repository" />
    </div>
    <section>
      <h2 class="mb-3 text-lg font-medium">Publications waiting for a decision</h2>
      <ErrorNotice v-if="unresolved.error.value" :error="unresolved.error.value" />
      <Skeleton v-else-if="!unresolved.data.value" class="h-20 w-full" />
      <UnresolvedList v-else :publications="unresolved.data.value" />
    </section>
    <section v-if="writer">
      <div class="mb-3 flex flex-wrap items-center gap-3">
        <h2 class="mr-auto text-lg font-medium">Jobs</h2>
        <Select :model-value="state ?? ANY" @update:model-value="state = pick($event)">
          <SelectTrigger class="w-40" aria-label="State"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem :value="ANY">Any state</SelectItem>
            <SelectItem v-for="s in JOB_STATES" :key="s" :value="s">{{
              statusLabel(s)
            }}</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <p class="mb-3 text-sm text-muted-foreground">
        Finished jobs stay 30 days; a failed run keeps its reason on its pull request after that.
      </p>
      <ErrorNotice v-if="jobs.error.value" :error="jobs.error.value" />
      <JobsTable :jobs="rows" />
      <LoadMore
        :more="jobs.hasNextPage.value"
        :loading="jobs.isFetchingNextPage.value"
        @next="jobs.fetchNextPage()"
      />
    </section>
  </div>
</template>
