<script setup lang="ts">
import { computed, ref } from 'vue'

import type { Schemas } from '@/modules/Api'
import {
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  ErrorNotice,
  StatusBadge,
  Textarea,
} from '@/modules/Ui'

import { useTourActions } from '../queries'

const props = defineProps<{ runId: string; view: Schemas['TourView'] }>()

const actions = useTourActions(
  computed(() => props.runId),
  computed(() => props.view.tour.tour_id),
)
const questions = ref<Record<string, string>>({})
const open = computed(() => props.view.session.state === 'IN_PROGRESS')
const revealed = (step: string) => props.view.session.revealed.includes(step)
const exchanges = (step: string) => props.view.exchanges.filter((e) => e.step_id === step)
const failure = computed(
  () => actions.reveal.error.value ?? actions.finish.error.value ?? actions.ask.error.value,
)

function ask(stepId: string) {
  const question = questions.value[stepId]?.trim()
  if (question) {
    actions.ask.mutate({ stepId, question }, { onSuccess: () => (questions.value[stepId] = '') })
  }
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-center gap-2 text-sm">
      <StatusBadge :value="view.tour.overview.coverage" />
      <StatusBadge :value="view.session.state" />
      <StatusBadge v-if="view.stale" value="STALE" />
      <span class="text-muted-foreground">
        {{ view.tour.overview.critical_findings.length }} high or critical findings ·
        {{ view.tour.overview.important_unknowns.length }} required checks unknown
      </span>
    </div>
    <p class="text-sm text-muted-foreground">
      Your progress is yours alone: it is never published and never approves the change.
    </p>
    <ErrorNotice v-if="failure" :error="failure" title="Not done" />

    <Card v-for="(step, i) in view.tour.steps" :key="step.id">
      <CardHeader>
        <CardTitle>{{ i + 1 }}. {{ step.title }}</CardTitle>
        <p class="text-sm text-muted-foreground">{{ step.purpose }}</p>
      </CardHeader>
      <CardContent class="space-y-3 text-sm">
        <p class="whitespace-pre-line">{{ step.explanation }}</p>
        <ul class="font-mono text-xs">
          <li v-for="c in step.code_refs" :key="`${c.path}:${c.start_line}`">
            {{ c.path }}:{{ c.start_line }}–{{ c.end_line }}
          </li>
        </ul>
        <div v-if="step.question" class="space-y-2 rounded-md bg-muted p-3">
          <p class="font-medium">{{ step.question.text }}</p>
          <p v-if="revealed(step.id)">{{ step.question.reveal }}</p>
          <Button
            v-else-if="open"
            size="sm"
            variant="outline"
            :disabled="actions.reveal.isPending.value"
            @click="actions.reveal.mutate(step.id)"
          >
            Reveal the answer
          </Button>
        </div>
        <div v-for="e in exchanges(step.id)" :key="e.id" class="border-l-2 pl-3">
          <p class="font-medium">{{ e.question }}</p>
          <p class="whitespace-pre-line">
            {{ e.reply.type === 'RISK_DECISION' ? `waiver ${e.reply.waiver_id}` : e.reply.text }}
          </p>
        </div>
        <form v-if="open" class="flex gap-2" @submit.prevent="ask(step.id)">
          <Textarea v-model="questions[step.id]" rows="1" placeholder="Ask about this step" />
          <Button type="submit" size="sm" variant="outline" :disabled="actions.ask.isPending.value">
            Ask
          </Button>
        </form>
      </CardContent>
    </Card>

    <div v-if="open" class="flex gap-2">
      <Button :disabled="actions.finish.isPending.value" @click="actions.finish.mutate('complete')">
        Done
      </Button>
      <Button
        variant="ghost"
        :disabled="actions.finish.isPending.value"
        @click="actions.finish.mutate('dismiss')"
      >
        Dismiss
      </Button>
    </div>
  </div>
</template>
