<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'

import { TourView, useTour } from '@/modules/Tours'
import { ErrorNotice, Skeleton } from '@/modules/Ui'

const route = useRoute()
const runId = computed(() => String(route.params.id))
const tour = useTour(runId)
</script>

<template>
  <div class="space-y-4">
    <h1 class="text-xl font-semibold">Tour</h1>
    <ErrorNotice v-if="tour.error.value" :error="tour.error.value" />
    <Skeleton v-else-if="tour.data.value === undefined" class="h-40 w-full" />
    <p v-else-if="tour.data.value === null" class="text-sm text-muted-foreground">
      This run has no tour.
    </p>
    <TourView v-else :run-id="runId" :view="tour.data.value" />
  </div>
</template>
