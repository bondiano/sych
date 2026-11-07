<script setup lang="ts">
import { computed } from 'vue'

import { formatMicroUsd } from '@/common/utilities/formatMicroUsd'
import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import { StatusBadge } from '@/modules/Ui'

import { formatSeconds, type CodeLinks } from '../report'

const props = defineProps<{
  run: Schemas['RunStatus']
  result?: Schemas['ReviewResult'] | null
  links?: CodeLinks
}>()

const usage = computed(() => props.result?.usage_summary)
const took = computed(() =>
  props.run.finished_at ? formatSeconds(props.run.finished_at - props.run.created_at) : undefined,
)
const commits = computed(() =>
  props.links
    ? [
        { label: 'Head', sha: props.links.head },
        { label: 'Target', sha: props.links.base },
      ]
    : [],
)
const number = new Intl.NumberFormat('en-US')
</script>

<template>
  <dl
    class="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 rounded-lg border bg-card p-4 text-sm [&>dd]:min-w-0 [&>dd]:text-right [&>dt]:text-muted-foreground"
  >
    <template v-if="result">
      <dt>Recommendation</dt>
      <dd><StatusBadge :value="result.recommendation" class="h-auto whitespace-normal" /></dd>
      <dt>Coverage</dt>
      <dd><StatusBadge :value="result.coverage" /></dd>
      <dt>Execution</dt>
      <dd><StatusBadge :value="result.execution_outcome" /></dd>
    </template>
    <template v-for="c in commits" :key="c.label">
      <dt>{{ c.label }}</dt>
      <dd>
        <a
          :href="`https://github.com/${links?.repo}/commit/${c.sha}`"
          target="_blank"
          rel="noopener noreferrer"
          class="font-mono text-xs underline decoration-muted-foreground/40 underline-offset-2 hover:decoration-current"
          >{{ c.sha.slice(0, 12) }}</a
        >
      </dd>
    </template>
    <dt>Started</dt>
    <dd>{{ formatTime(run.created_at) }}</dd>
    <template v-if="took">
      <dt>Took</dt>
      <dd class="tabular-nums">{{ took }}</dd>
    </template>
    <dt>Cost</dt>
    <dd class="tabular-nums">
      {{ formatMicroUsd(run.usage.cost_known_micro_usd) }} known
      <span v-if="run.usage.unknown_cost_attempts" class="block text-muted-foreground">
        + {{ run.usage.unknown_cost_attempts }} of unknown cost
      </span>
    </dd>
    <dt>Model calls</dt>
    <dd class="tabular-nums">{{ run.usage.model_attempts }}</dd>
    <template v-if="usage">
      <dt>Tokens in / out</dt>
      <dd class="tabular-nums">
        {{ number.format(usage.input_tokens) }} / {{ number.format(usage.output_tokens) }}
      </dd>
    </template>
    <template v-if="result?.scope_manifest.requested_profiles.length">
      <dt>Profiles</dt>
      <dd>{{ result.scope_manifest.requested_profiles.join(', ') }}</dd>
    </template>
    <template v-if="result">
      <dt>Engine</dt>
      <dd class="break-words">{{ result.provenance.engine_version }}</dd>
      <dt>Result</dt>
      <dd>
        <a
          :href="run.result_url"
          class="font-mono text-xs underline decoration-muted-foreground/40 underline-offset-2 hover:decoration-current"
          :title="result.result_id"
          >{{ result.result_id.slice(0, 8) }} JSON</a
        >
      </dd>
    </template>
  </dl>
</template>
