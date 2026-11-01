<script setup lang="ts">
import { computed } from 'vue'

import { formatTime } from '@/common/utilities/formatTime'
import type { Schemas } from '@/modules/Api'
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  statusLabel,
} from '@/modules/Ui'

const props = defineProps<{ connection: Schemas['ConnectionView'] }>()

// A self-review is done by the pinned build only; another build refuses to start it.
const drift = computed(
  () =>
    props.connection.self_review &&
    props.connection.reviewer_commit !== props.connection.build_commit,
)
const flags = computed(() =>
  [
    props.connection.environment,
    props.connection.publish ? 'publishes' : 'does not publish',
    props.connection.checks ? 'check runs' : 'no check runs',
    props.connection.self_review ? 'self-review' : '',
  ].filter(Boolean),
)
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle class="font-mono">{{ connection.id }}</CardTitle>
      <CardDescription>
        App {{ connection.app_id }} as {{ connection.bot_login }} · {{ flags.join(' · ') }}
      </CardDescription>
    </CardHeader>
    <CardContent class="space-y-3 text-sm">
      <div>
        <span class="text-muted-foreground">Repositories</span>
        <span class="ml-2 font-mono text-xs">{{ connection.repositories.join(', ') }}</span>
      </div>
      <div>
        <span class="text-muted-foreground">Build</span>
        <span class="ml-2 font-mono text-xs">{{ connection.build_commit }}</span>
        <template v-if="connection.reviewer_commit">
          <span class="ml-4 text-muted-foreground">Pinned reviewer</span>
          <span class="ml-2 font-mono text-xs">{{ connection.reviewer_commit }}</span>
        </template>
      </div>
      <Alert v-if="drift" variant="destructive">
        <AlertTitle>Reviewer pin and build differ</AlertTitle>
        <AlertDescription>A self-review runs only on the pinned reviewer build.</AlertDescription>
      </Alert>
      <Alert v-if="connection.last_scm_read_failure" variant="destructive">
        <AlertTitle>
          GitHub read failed {{ formatTime(connection.last_scm_read_failure.at) }}
        </AlertTitle>
        <AlertDescription>
          {{ connection.last_scm_read_failure.detail }} —
          <RouterLink
            :to="`/changes/${connection.last_scm_read_failure.change_id}`"
            class="underline"
          >
            pull request
          </RouterLink>
        </AlertDescription>
      </Alert>
      <div>
        <span class="text-muted-foreground">Webhooks, 30 days</span>
        <span v-if="!connection.deliveries.length" class="ml-2">none received</span>
        <span v-for="d in connection.deliveries" :key="d.key" class="ml-2">
          {{ statusLabel(d.key) }} {{ d.count }}
        </span>
      </div>
    </CardContent>
  </Card>
</template>
