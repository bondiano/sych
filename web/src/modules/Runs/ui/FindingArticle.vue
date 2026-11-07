<script setup lang="ts">
import { computed } from 'vue'

import type { Schemas } from '@/modules/Api'
import { StatusBadge } from '@/modules/Ui'

import { agentPrompt, categoryLabel, type CodeLinks } from '../report'
import CodePlace from './CodePlace.vue'
import CopyPrompt from './CopyPrompt.vue'
import ProseText from './ProseText.vue'

const props = defineProps<{
  finding: Schemas['Finding']
  links?: CodeLinks
  /** Its row in the change's findings: state, history and feedback. */
  tracked?: Schemas['FindingSummary']
}>()

const when = computed(() => props.finding.triggering_conditions.join('; '))
</script>

<template>
  <article class="space-y-4 p-5">
    <header class="space-y-2">
      <div class="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
        <StatusBadge :value="finding.severity" />
        <span class="text-muted-foreground">{{ categoryLabel(finding.category) }}</span>
        <span v-if="finding.rule_id" class="font-mono text-xs text-muted-foreground">
          {{ finding.rule_id }}
        </span>
        <StatusBadge v-if="tracked" :value="tracked.state" />
      </div>
      <h3 class="text-base font-medium text-balance"><ProseText :text="finding.title" /></h3>
      <ul v-if="finding.anchors.length" class="space-y-0.5">
        <li v-for="a in finding.anchors" :key="`${a.side}:${a.path}:${a.start_line}`">
          <CodePlace :code="a" :links="links" />
        </li>
      </ul>
    </header>

    <div class="max-w-prose space-y-4 text-sm leading-relaxed">
      <p class="whitespace-pre-line"><ProseText :text="finding.explanation" /></p>

      <dl v-if="when || finding.consequence" class="grid gap-4 sm:grid-cols-2">
        <div v-if="when">
          <dt class="font-medium">When</dt>
          <dd class="text-muted-foreground"><ProseText :text="when" /></dd>
        </div>
        <div v-if="finding.consequence">
          <dt class="font-medium">Consequence</dt>
          <dd class="text-muted-foreground"><ProseText :text="finding.consequence" /></dd>
        </div>
      </dl>

      <section v-if="finding.also?.length">
        <h4 class="font-medium">Also on these lines</h4>
        <ul class="mt-1 space-y-1">
          <li v-for="a in finding.also" :key="a.title">
            <StatusBadge :value="a.severity" />
            {{ categoryLabel(a.category) }}: <ProseText :text="a.title" />{{ ' '
            }}<span v-if="a.consequence" class="text-muted-foreground"
              >— <ProseText :text="a.consequence"
            /></span>
          </li>
        </ul>
      </section>

      <p v-if="finding.suggested_action">
        <span class="font-medium">Suggested.</span>{{ ' '
        }}<ProseText :text="finding.suggested_action" />
      </p>
      <p v-if="finding.validation_expectation">
        <span class="font-medium">To check.</span>{{ ' '
        }}<ProseText :text="finding.validation_expectation" />
      </p>
    </div>

    <figure v-if="finding.suggested_change" class="space-y-1">
      <figcaption class="text-sm text-muted-foreground">
        Suggested code for <CodePlace :code="finding.suggested_change.code" />
      </figcaption>
      <pre
        v-if="finding.suggested_change.replacement"
        class="overflow-x-auto rounded-md bg-muted p-3 font-mono text-xs leading-relaxed"
        >{{ finding.suggested_change.replacement }}</pre>
      <p v-else class="text-sm">Delete these lines.</p>
    </figure>

    <ul v-if="finding.limitations?.length" class="max-w-prose space-y-1 text-sm">
      <li v-for="l in finding.limitations" :key="l" class="text-muted-foreground">
        <span class="font-medium text-foreground">Limitation.</span>{{ ' ' }}<ProseText :text="l" />
      </li>
    </ul>

    <footer class="flex flex-wrap items-center gap-x-4 gap-y-2 text-sm">
      <CopyPrompt :text="agentPrompt([finding])" label="Copy prompt for an AI agent" />
      <RouterLink
        v-if="tracked"
        :to="`/findings/${tracked.id}`"
        class="underline decoration-muted-foreground/40 underline-offset-2 hover:decoration-current"
      >
        History and feedback
      </RouterLink>
    </footer>
  </article>
</template>
