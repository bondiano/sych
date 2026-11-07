<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import { toId } from '@/common/utilities/toId'
import { PlaygroundRuns, usePlaygroundRuns } from '@/modules/Playground'
import { RepositorySelect } from '@/modules/Session'
import { ErrorNotice, Skeleton } from '@/modules/Ui'

const repository = useQueryParam('repository')
const runs = usePlaygroundRuns(computed(() => toId(repository.value)))
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="mr-auto text-xl font-semibold">Playground</h1>
      <RepositorySelect v-model="repository" />
    </div>
    <p class="text-sm text-muted-foreground">
      Tries a maintainer started from a pull request's page. They review the pull request as it is
      now with the current policy, publish nothing, and stay out of findings and analytics; their
      spend counts against the day's playground budget.
    </p>
    <ErrorNotice v-if="runs.error.value" :error="runs.error.value" />
    <Skeleton v-else-if="!runs.data.value" class="h-40 w-full" />
    <PlaygroundRuns v-else :runs="runs.data.value" />
  </div>
</template>
