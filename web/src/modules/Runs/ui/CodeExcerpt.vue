<script setup lang="ts">
import { ExternalLink } from '@lucide/vue'
import { computed, ref } from 'vue'

import type { Schemas } from '@/modules/Api'
import { Button } from '@/modules/Ui'

import { highlightLines, type HighlightSpan } from '../highlight'
import { strings, type Lang } from '../i18n'
import { codeLink, place, safeText, type AnyCodeLinks } from '../report'

const COLLAPSE_THRESHOLD = 80
const CONTEXT_PAD = 12

const props = defineProps<{
  reference: Schemas['CodeRef']
  excerpt?: Schemas['ReportExcerpt'] | null
  links?: AnyCodeLinks
  language?: Lang
}>()

const expanded = ref(false)
const t = computed(() => strings(props.language))
const href = computed(() => codeLink(props.reference, props.links))
const providerLabel = computed(() =>
  props.links?.provider === 'gitlab' ? t.value.openOnGitlab : t.value.openOnGithub,
)

interface NumberedLine {
  lineNumber: number
  marked: boolean
  spans: HighlightSpan[]
}

const allLines = computed<NumberedLine[]>(() => {
  const ex = props.excerpt
  if (!ex) {
    return []
  }

  const tokenized = highlightLines(safeText(ex.text), props.reference.path)
  return tokenized.map((spans, idx) => {
    const lineNumber = ex.first_line + idx
    const marked =
      lineNumber >= props.reference.start_line && lineNumber <= props.reference.end_line
    return { lineNumber, marked, spans }
  })
})

const canCollapse = computed(() => allLines.value.length > COLLAPSE_THRESHOLD)

const visibleLines = computed<NumberedLine[]>(() => {
  if (!canCollapse.value || expanded.value) {
    return allLines.value
  }

  const first = props.excerpt?.first_line ?? 1
  const startIdx = Math.max(0, props.reference.start_line - first - CONTEXT_PAD)
  const endIdx = Math.min(allLines.value.length, props.reference.end_line - first + CONTEXT_PAD + 1)
  return allLines.value.slice(startIdx, Math.max(startIdx + 20, endIdx))
})
</script>

<template>
  <div class="overflow-hidden rounded-lg border bg-card">
    <div
      class="flex flex-wrap items-center justify-between gap-2 border-b bg-muted/50 px-3 py-2 text-xs"
    >
      <div class="flex flex-wrap items-center gap-2 font-mono">
        <span class="font-medium break-all text-foreground">{{ place(reference) }}</span>
        <span class="rounded bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground">
          {{ reference.side }}
        </span>
      </div>
      <a
        v-if="href"
        :href="href"
        target="_blank"
        rel="noopener noreferrer"
        class="inline-flex items-center gap-1 text-muted-foreground underline decoration-muted-foreground/40 underline-offset-2 hover:text-foreground hover:decoration-current print:hidden"
      >
        {{ providerLabel }}
        <ExternalLink class="size-3" aria-hidden="true" />
      </a>
    </div>

    <div v-if="excerpt && visibleLines.length" class="overflow-x-auto">
      <table class="w-full border-collapse font-mono text-xs leading-relaxed">
        <tbody>
          <tr
            v-for="row in visibleLines"
            :key="row.lineNumber"
            :class="row.marked ? 'bg-amber-500/12 dark:bg-amber-400/12' : undefined"
          >
            <td
              class="w-6 px-1.5 text-center font-semibold select-none"
              :class="row.marked ? 'text-primary' : 'text-transparent'"
              aria-hidden="true"
            >
              {{ row.marked ? '>' : ' ' }}
            </td>
            <td
              class="w-12 pr-3 text-right text-muted-foreground tabular-nums select-none"
              :class="row.marked ? 'font-semibold text-foreground' : undefined"
            >
              {{ row.lineNumber }}
            </td>
            <td class="pr-4 whitespace-pre text-foreground">
              <template v-for="(span, i) in row.spans" :key="i">
                <span :class="span.kind ? `sych-tok-${span.kind}` : undefined">{{
                  span.text
                }}</span>
              </template>
            </td>
          </tr>
        </tbody>
      </table>
      <div v-if="canCollapse" class="border-t bg-muted/30 px-3 py-1.5 print:hidden">
        <Button variant="ghost" size="sm" type="button" @click="expanded = !expanded">
          {{ expanded ? t.collapseLines : t.expandLines(allLines.length) }}
        </Button>
      </div>
    </div>

    <p v-else class="px-3 py-2.5 text-xs text-muted-foreground">
      {{ t.sourceUnavailable }}
    </p>
  </div>
</template>
