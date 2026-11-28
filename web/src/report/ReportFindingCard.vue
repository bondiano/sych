<script setup lang="ts">
import { computed } from 'vue'

import type { Schemas } from '@/modules/Api'
import {
  agentPrompt,
  categoryLabel,
  CodeExcerpt,
  CodePlace,
  CopyPrompt,
  DiffView,
  ProseText,
  safeText,
  strings,
  type AnyCodeLinks,
  type Lang,
} from '@/modules/Runs'
import { StatusBadge } from '@/modules/Ui'

const props = defineProps<{
  finding: Schemas['ReportFinding']
  links?: AnyCodeLinks
  language?: Lang
}>()

const t = computed(() => strings(props.language))
const when = computed(() => props.finding.triggering_conditions.join('; '))
const explanationProse = computed(() => {
  const marker = '\n\n```text\n'
  const idx = props.finding.explanation.indexOf(marker)
  return idx >= 0 ? props.finding.explanation.slice(0, idx) : props.finding.explanation
})
const distinctRemediation = computed(() => {
  const rem = props.finding.remediation
  return rem && rem !== props.finding.suggested_action ? rem : undefined
})
const primaryPath = computed(() => props.finding.anchors[0]?.reference.path)
</script>

<template>
  <article :id="`finding-${finding.id}`" class="scroll-mt-6 space-y-4 p-5">
    <header class="space-y-2">
      <div class="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
        <StatusBadge :value="finding.severity" />
        <span class="text-muted-foreground">{{ categoryLabel(finding.category, language) }}</span>
        <span v-if="finding.rule_id" class="font-mono text-xs text-muted-foreground">
          {{ safeText(finding.rule_id) }}
        </span>
        <StatusBadge v-if="finding.lifecycle !== 'OPEN'" :value="finding.lifecycle" />
      </div>
      <h4 class="text-base font-medium text-balance">
        <ProseText :text="finding.title" />
      </h4>
      <ul v-if="finding.anchors.length" class="space-y-0.5">
        <li
          v-for="a in finding.anchors"
          :key="`${a.reference.side}:${a.reference.path}:${a.reference.start_line}-${a.reference.end_line}`"
        >
          <CodePlace :code="a.reference" :links="links" />
        </li>
      </ul>
    </header>

    <div class="max-w-[72ch] space-y-4 text-sm leading-relaxed">
      <p class="whitespace-pre-line"><ProseText :text="explanationProse" /></p>

      <pre
        v-if="finding.picture"
        class="overflow-x-auto rounded-md border bg-muted/50 p-3 font-mono text-xs leading-snug"
        >{{ safeText(finding.picture) }}</pre>

      <dl v-if="when || finding.consequence" class="grid gap-4 sm:grid-cols-2">
        <div v-if="when">
          <dt class="font-medium">{{ t.when }}</dt>
          <dd class="text-muted-foreground"><ProseText :text="when" /></dd>
        </div>
        <div v-if="finding.consequence">
          <dt class="font-medium">{{ t.consequence }}</dt>
          <dd class="text-muted-foreground"><ProseText :text="finding.consequence" /></dd>
        </div>
      </dl>

      <section v-if="finding.also?.length">
        <h5 class="font-medium">{{ t.alsoOnLines }}</h5>
        <ul class="mt-1 space-y-1">
          <li v-for="a in finding.also" :key="a.title">
            <StatusBadge :value="a.severity" />
            {{ categoryLabel(a.category, language) }}: <ProseText :text="a.title" />{{ ' '
            }}<span v-if="a.consequence" class="text-muted-foreground"
              >: <ProseText :text="a.consequence"
            /></span>
          </li>
        </ul>
      </section>

      <p v-if="finding.suggested_action">
        <span class="font-medium">{{ t.suggested }}</span>{{ ' '
        }}<ProseText :text="finding.suggested_action" />
      </p>
      <p v-if="distinctRemediation">
        <span class="font-medium">{{ t.remediation }}</span>{{ ' '
        }}<ProseText :text="distinctRemediation" />
      </p>
      <p v-if="finding.validation_expectation">
        <span class="font-medium">{{ t.toCheck }}</span>{{ ' '
        }}<ProseText :text="finding.validation_expectation" />
      </p>
    </div>

    <div
      v-for="(anchor, idx) in finding.anchors"
      :key="`code-${idx}-${anchor.reference.path}-${anchor.reference.start_line}`"
      class="space-y-3"
    >
      <CodeExcerpt
        :reference="anchor.reference"
        :excerpt="anchor.excerpt"
        :links="links"
        :language="language"
      />
      <DiffView
        v-if="anchor.diff"
        :diff="anchor.diff"
        :path="anchor.reference.path"
        :title="t.changeDiff"
        :language="language"
      />
    </div>

    <div v-if="finding.suggested_patch || finding.fix_note" class="space-y-2">
      <DiffView
        v-if="finding.suggested_patch"
        :diff="finding.suggested_patch"
        :path="primaryPath"
        :title="t.suggestedFix"
        :language="language"
      />
      <p v-if="finding.fix_note" class="text-xs text-muted-foreground">
        <ProseText :text="finding.fix_note" />
      </p>
    </div>

    <ul v-if="finding.limitations?.length" class="max-w-[72ch] space-y-1 text-sm">
      <li v-for="l in finding.limitations" :key="l" class="text-muted-foreground">
        <span class="font-medium text-foreground">{{ t.limitation }}</span>{{ ' '
        }}<ProseText :text="l" />
      </li>
    </ul>

    <footer class="flex flex-wrap items-start gap-3 pt-1 text-sm print:hidden">
      <CopyPrompt
        :text="agentPrompt([finding], language)"
        :label="t.copyPromptOne"
        :language="language"
      />
      <CopyPrompt
        v-if="finding.suggested_patch"
        :text="finding.suggested_patch"
        :label="t.copyPatchOne"
        :language="language"
      />
    </footer>
  </article>
</template>
