<script setup lang="ts">
import { ref } from 'vue'

import type { Schemas } from '@/modules/Api'
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  ErrorNotice,
  Input,
} from '@/modules/Ui'

import { useDecideSuggestion } from '../queries'

const props = defineProps<{
  suggestion: Schemas['Suggestion']
  repositoryId: number
  canDecide: boolean
}>()

const { mutate, isPending, error } = useDecideSuggestion()
const pullRequest = ref('')
const copied = ref(false)

async function copy() {
  await navigator.clipboard.writeText(props.suggestion.yaml)
  copied.value = true
}

function record(state: 'DISMISSED' | 'DONE') {
  mutate({
    repositoryId: props.repositoryId,
    fingerprint: props.suggestion.fingerprint,
    body: { state, pull_request: state === 'DONE' && pullRequest.value ? pullRequest.value : null },
  })
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="flex flex-wrap items-center gap-2 text-base">
        <span class="font-mono">{{ suggestion.rule_id }}</span>
        <Badge variant="outline">{{
          suggestion.kind === 'EXCEPTION'
            ? `exception for ${suggestion.paths.join(', ')}`
            : 'not required'
        }}</Badge>
        <Badge v-if="suggestion.state !== 'PENDING'" variant="secondary">{{
          suggestion.state === 'DONE' ? 'applied' : 'dismissed'
        }}</Badge>
      </CardTitle>
      <CardDescription>
        {{ suggestion.against }} of {{ suggestion.judged }} judgements of findings were
        {{ suggestion.kind === 'EXCEPTION' ? "false positive or won't fix" : 'false positive' }}
        (rule version {{ suggestion.rule_version }}). Findings:
        <template v-for="(f, i) in suggestion.finding_ids" :key="f"
          ><RouterLink :to="`/findings/${f}`" class="underline">#{{ f }}</RouterLink
          >{{ i < suggestion.finding_ids.length - 1 ? ', ' : '' }}</template
        >
        <a v-if="suggestion.pull_request" :href="suggestion.pull_request" class="ml-1 underline"
          >pull request</a
        >
      </CardDescription>
    </CardHeader>
    <CardContent class="space-y-3">
      <pre class="overflow-x-auto rounded-lg border p-3 text-xs">{{ suggestion.yaml }}</pre>
      <div class="flex flex-wrap items-center gap-2">
        <Button variant="outline" size="sm" @click="copy">{{
          copied ? 'Copied' : 'Copy YAML'
        }}</Button>
        <template v-if="canDecide && suggestion.state === 'PENDING'">
          <Input
            v-model="pullRequest"
            class="w-80"
            placeholder="https://github.com/owner/name/pull/1"
            aria-label="Pull request that applies it"
          />
          <Button size="sm" :disabled="isPending" @click="record('DONE')">Applied</Button>
          <Button variant="ghost" size="sm" :disabled="isPending" @click="record('DISMISSED')"
            >Dismiss</Button
          >
        </template>
      </div>
      <ErrorNotice v-if="error" :error="error" title="Not recorded" />
    </CardContent>
  </Card>
</template>
