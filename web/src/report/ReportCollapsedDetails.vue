<script setup lang="ts">
import { computed } from 'vue'

import type { Schemas } from '@/modules/Api'
import {
  CodePlace,
  ProseText,
  RunDisclosure,
  RunScope,
  safeText,
  strings,
  type AnyCodeLinks,
  type Lang,
} from '@/modules/Runs'
import {
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/modules/Ui'

const props = defineProps<{
  view: Schemas['ReportView']
  links?: AnyCodeLinks
  language?: Lang
}>()

const t = computed(() => strings(props.language))

const policySourceLabel = computed(() => {
  const src = props.view.policy?.snapshot.source
  if (!src) {
    return ''
  }

  if (src.kind === 'engine_defaults') {
    return 'engine defaults'
  }

  return `${safeText(src.ref_name)} @ ${safeText(src.commit).slice(0, 17)}`
})

const sortedObligations = computed(() =>
  [...(props.view.obligations ?? [])].sort(
    (a, b) =>
      Number(b.status === 'UNKNOWN') - Number(a.status === 'UNKNOWN') ||
      Number(b.required) - Number(a.required),
  ),
)
</script>

<template>
  <section class="space-y-2">
    <RunDisclosure v-if="view.policy" :title="t.policyHeading">
      <div class="space-y-4 p-4 text-sm">
        <dl class="grid gap-2 sm:grid-cols-2">
          <div>
            <dt class="text-xs text-muted-foreground">{{ t.policySource }}</dt>
            <dd class="font-mono text-xs break-all">{{ policySourceLabel }}</dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">Snapshot</dt>
            <dd class="font-mono text-xs break-all">{{ view.policy.snapshot.id }}</dd>
          </div>
        </dl>

        <div v-if="view.policy.provenance.length" class="space-y-1">
          <ul class="space-y-1 font-mono text-xs text-muted-foreground">
            <li v-for="(p, idx) in view.policy.provenance" :key="idx" class="break-all">
              <span class="text-foreground">{{ safeText(p.layer) }}</span
              >: {{ safeText(p.origin) }}
              <span v-if="p.sha256">({{ p.sha256.slice(0, 19) }})</span>
            </li>
          </ul>
        </div>

        <div
          v-if="
            view.policy.proposed_files?.length ||
            view.policy.proposed_rules?.length ||
            view.policy.head_errors?.length
          "
          class="space-y-1"
        >
          <p class="text-xs font-medium">{{ t.policyProposed }}</p>
          <ul class="space-y-0.5 font-mono text-xs text-muted-foreground">
            <li v-for="f in view.policy.proposed_files" :key="f">{{ safeText(f) }}</li>
            <li v-for="r in view.policy.proposed_rules" :key="r">{{ safeText(r) }}</li>
            <li v-for="e in view.policy.head_errors" :key="e" class="text-destructive">
              {{ safeText(e) }}
            </li>
          </ul>
        </div>
      </div>
    </RunDisclosure>

    <RunDisclosure
      v-if="sortedObligations.length"
      :title="t.obligationsHeading"
      :count="sortedObligations.length"
    >
      <div class="p-4">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Status</TableHead>
              <TableHead>Rule</TableHead>
              <TableHead>Question</TableHead>
              <TableHead>Details</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="o in sortedObligations" :key="o.id">
              <TableCell><StatusBadge :value="o.status" /></TableCell>
              <TableCell class="font-mono text-xs">
                {{ safeText(o.rule_id) }} v{{ o.rule_version }}
                <span v-if="o.required" class="text-muted-foreground"> ({{ t.requiredTag }}) </span>
              </TableCell>
              <TableCell class="max-w-md whitespace-normal">
                <ProseText :text="o.question" />
              </TableCell>
              <TableCell class="max-w-sm text-xs whitespace-normal text-muted-foreground">
                <template v-if="o.status === 'UNKNOWN'">
                  {{ safeText(o.reason_code) }}: <ProseText :text="o.next_evidence_needed" />
                </template>
                <template v-else-if="o.status === 'NOT_APPLICABLE'">
                  <ProseText :text="o.basis" />
                </template>
                <template v-else>
                  <ProseText :text="o.acceptance_criterion" />
                </template>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
    </RunDisclosure>

    <RunDisclosure
      v-if="view.evidence?.length"
      :title="t.evidenceHeading"
      :count="view.evidence.length"
    >
      <ul class="divide-y text-sm">
        <li v-for="ev in view.evidence" :key="ev.id" class="space-y-1.5 p-4">
          <div class="flex flex-wrap items-center gap-2 text-xs">
            <span class="font-mono text-muted-foreground">{{ ev.id.slice(0, 8) }}</span>
            <StatusBadge :value="ev.kind" />
            <StatusBadge :value="ev.validity" />
            <StatusBadge :value="ev.completeness" />
            <span class="text-muted-foreground">
              {{ safeText(ev.producer.name) }} {{ safeText(ev.producer.version) }}
            </span>
          </div>
          <ul v-if="ev.code_refs?.length" class="space-y-0.5">
            <li
              v-for="r in ev.code_refs"
              :key="`${r.side}:${r.path}:${r.start_line}-${r.end_line}`"
            >
              <CodePlace :code="r" :links="links" />
            </li>
          </ul>
          <p v-for="lim in ev.limitations" :key="lim" class="text-xs text-muted-foreground">
            <ProseText :text="lim" />
          </p>
        </li>
      </ul>
    </RunDisclosure>

    <RunDisclosure
      v-if="view.header.scope.units.length"
      :title="t.scopeHeading"
      :count="view.header.scope.units.length"
    >
      <RunScope :scope="view.header.scope" />
    </RunDisclosure>
  </section>
</template>
