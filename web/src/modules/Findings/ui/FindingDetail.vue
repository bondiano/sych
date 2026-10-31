<script setup lang="ts">
import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import { StatusBadge } from '@/modules/Ui'

defineProps<{ detail: Schemas['FindingDetail'] }>()
</script>

<template>
  <div class="space-y-6">
    <div class="flex flex-wrap items-center gap-2">
      <StatusBadge :value="detail.summary.severity" />
      <StatusBadge :value="detail.summary.state" />
      <span class="text-sm text-muted-foreground">{{ detail.summary.category.toLowerCase() }}</span>
      <span v-if="detail.summary.rule_id" class="font-mono text-xs">{{
        detail.summary.rule_id
      }}</span>
    </div>

    <section v-if="detail.finding" class="space-y-3 text-sm">
      <p>{{ detail.finding.explanation }}</p>
      <p><span class="font-medium">Consequence.</span> {{ detail.finding.consequence }}</p>
      <ul v-if="detail.finding.triggering_conditions.length" class="list-disc pl-5">
        <li v-for="c in detail.finding.triggering_conditions" :key="c">{{ c }}</li>
      </ul>
      <p><span class="font-medium">Suggested.</span> {{ detail.finding.suggested_action }}</p>
      <ul class="font-mono text-xs">
        <li v-for="a in detail.finding.anchors" :key="`${a.path}:${a.start_line}`">
          {{ a.path }}:{{ a.start_line }}–{{ a.end_line }} ({{ a.side.toLowerCase() }})
        </li>
      </ul>
    </section>
    <p v-else class="text-sm text-muted-foreground">
      The result that stated this finding was deleted by retention; its history stays.
    </p>

    <section>
      <h3 class="mb-2 font-medium">History</h3>
      <ol class="space-y-1 text-sm">
        <li v-for="(e, i) in detail.history" :key="i" class="flex flex-wrap gap-2">
          <span class="text-muted-foreground">{{ formatTime(e.at) }}</span>
          <StatusBadge :value="e.to_state" />
          <span>{{ e.reason }}</span>
          <span v-if="e.actor" class="text-muted-foreground">by {{ e.actor }}</span>
        </li>
      </ol>
    </section>

    <section v-if="detail.feedback.length">
      <h3 class="mb-2 font-medium">Feedback</h3>
      <ul class="space-y-1 text-sm">
        <li v-for="(f, i) in detail.feedback" :key="i">
          <StatusBadge :value="f.judgement" /> {{ f.rationale }}
          <span class="text-muted-foreground">— {{ f.actor }}, {{ formatTime(f.at) }}</span>
        </li>
      </ul>
    </section>
  </div>
</template>
