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

import ProseText from './ProseText.vue'

const props = defineProps<{ result: Schemas['ReviewResult'] }>()

// What was not checked comes first: required unknowns before the rest.
const obligations = computed(() =>
  [...props.result.obligations].sort(
    (a, b) =>
      Number(b.status === 'UNKNOWN') - Number(a.status === 'UNKNOWN') ||
      Number(b.required) - Number(a.required),
  ),
)
// Only the change's own marker kept these from the reviewer: a reader must look at them.
const hidden = computed(() =>
  props.result.excluded_or_unreviewed.filter((g) => g.reason === 'GENERATED_IN_CHANGE'),
)
const excluded = computed(() =>
  props.result.excluded_or_unreviewed.filter((g) => g.reason !== 'GENERATED_IN_CHANGE'),
)
const gaps = computed(() => excluded.value.length + hidden.value.length)
</script>

<template>
  <div class="space-y-6">
    <section v-if="obligations.length">
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
            <TableCell class="max-w-md whitespace-normal"
              ><ProseText :text="o.question"
            /></TableCell>
            <TableCell class="max-w-sm text-sm whitespace-normal text-muted-foreground">
              <template v-if="o.status === 'UNKNOWN'">
                {{ o.reason_code }}: <ProseText :text="o.next_evidence_needed" />
              </template>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </section>

    <section v-if="gaps || result.unresolved_questions.length">
      <h3 class="mb-2 font-medium">Not checked</h3>
      <ul class="space-y-1 text-sm">
        <li v-for="g in hidden" :key="g.path">
          <span class="font-medium">Hidden by this change:</span>
          <span class="ml-1 font-mono text-xs break-all">{{ g.path }}</span>
          <span class="text-muted-foreground">
            — its own content marks it generated; the reviewer did not see it, check it by hand
          </span>
        </li>
        <li v-for="g in excluded" :key="`${g.path}-${g.reason}`">
          <span class="font-mono text-xs break-all">{{ g.path }}</span>
          <span class="text-muted-foreground"> — {{ g.reason }}</span>
        </li>
        <li v-for="q in result.unresolved_questions" :key="q" class="text-muted-foreground">
          <ProseText :text="q" />
        </li>
      </ul>
    </section>
  </div>
</template>
