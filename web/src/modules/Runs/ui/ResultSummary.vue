<script setup lang="ts">
import { computed } from 'vue'

import type { Schemas } from '@/modules/Api'
import {
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/modules/Ui'

const props = defineProps<{ result: Schemas['ReviewResult'] }>()

// What was not checked comes first: required unknowns before the rest.
const obligations = computed(() =>
  [...props.result.obligations].sort(
    (a, b) =>
      Number(b.status === 'UNKNOWN') - Number(a.status === 'UNKNOWN') ||
      Number(b.required) - Number(a.required),
  ),
)
</script>

<template>
  <div class="space-y-6">
    <div class="flex flex-wrap items-center gap-2 text-sm">
      <StatusBadge :value="result.recommendation" />
      <StatusBadge :value="result.coverage" />
      <StatusBadge :value="result.execution_outcome" />
      <span class="text-muted-foreground">revision {{ result.review_revision }}</span>
    </div>

    <section>
      <h3 class="mb-2 font-medium">Obligations</h3>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Status</TableHead>
            <TableHead>Rule</TableHead>
            <TableHead>Question</TableHead>
            <TableHead>Why open</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="o in obligations" :key="o.id">
            <TableCell><StatusBadge :value="o.status" /></TableCell>
            <TableCell class="font-mono text-xs">
              {{ o.rule_id }}<span v-if="o.required" class="text-muted-foreground"> required</span>
            </TableCell>
            <TableCell class="max-w-md whitespace-normal">{{ o.question }}</TableCell>
            <TableCell class="max-w-sm text-sm whitespace-normal text-muted-foreground">
              <template v-if="o.status === 'UNKNOWN'">
                {{ o.reason_code }}: {{ o.next_evidence_needed }}
              </template>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </section>

    <section v-if="result.excluded_or_unreviewed.length || result.unresolved_questions.length">
      <h3 class="mb-2 font-medium">Not checked</h3>
      <ul class="space-y-1 text-sm">
        <li v-for="g in result.excluded_or_unreviewed" :key="`${g.path}-${g.reason}`">
          <span class="font-mono text-xs">{{ g.path }}</span>
          <span class="text-muted-foreground"> — {{ g.reason }}</span>
        </li>
        <li v-for="q in result.unresolved_questions" :key="q" class="text-muted-foreground">
          {{ q }}
        </li>
      </ul>
    </section>
  </div>
</template>
