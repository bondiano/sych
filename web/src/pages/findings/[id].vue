<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'

import { FeedbackForm, FindingDetail, useFinding } from '@/modules/Findings'
import { can, useMe } from '@/modules/Session'
import { Card, CardContent, CardHeader, CardTitle, ErrorNotice, Skeleton } from '@/modules/Ui'
import { WaiverForm } from '@/modules/Waivers'

const route = useRoute()
const id = computed(() => Number(route.params.id))
const finding = useFinding(id)
const { data: me } = useMe()
const allowed = (role: 'triage' | 'maintainer') => {
  const repository = finding.data.value?.summary.repository.id
  return repository !== undefined && can(me.value, role, repository)
}
const triage = computed(() => allowed('triage'))
// A waiver accepts a rule; a model finding has none and is settled by feedback instead.
const waivable = computed(() => !!finding.data.value?.summary.rule_id && allowed('maintainer'))
</script>

<template>
  <ErrorNotice v-if="finding.error.value" :error="finding.error.value" />
  <Skeleton v-else-if="!finding.data.value" class="h-40 w-full" />
  <div v-else class="grid gap-8 lg:grid-cols-[1fr_20rem]">
    <div class="space-y-2">
      <h1 class="text-xl font-semibold">
        {{ finding.data.value.summary.title ?? finding.data.value.finding?.title ?? 'Finding' }}
      </h1>
      <RouterLink
        :to="`/changes/${finding.data.value.summary.change_id}`"
        class="text-sm text-muted-foreground hover:underline"
      >
        {{ finding.data.value.summary.repository.full_name }}#{{
          finding.data.value.summary.number
        }}
      </RouterLink>
      <FindingDetail :detail="finding.data.value" class="pt-4" />
    </div>
    <div v-if="triage || waivable" class="space-y-6">
      <Card v-if="triage">
        <CardHeader><CardTitle>Your feedback</CardTitle></CardHeader>
        <CardContent><FeedbackForm :finding-id="id" /></CardContent>
      </Card>
      <Card v-if="waivable">
        <CardHeader><CardTitle>Waive this rule</CardTitle></CardHeader>
        <CardContent>
          <WaiverForm
            :change-id="finding.data.value.summary.change_id"
            :rule-id="finding.data.value.summary.rule_id ?? ''"
            :path="finding.data.value.summary.path"
          />
        </CardContent>
      </Card>
    </div>
  </div>
</template>
