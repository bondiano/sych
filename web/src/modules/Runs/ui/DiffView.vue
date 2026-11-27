<script setup lang="ts">
import { computed } from 'vue'

import { parseUnifiedDiff, type DiffRow } from '../diff'
import { highlightSingleLine, type HighlightSpan } from '../highlight'
import { strings, type Lang } from '../i18n'
import { safeText } from '../report'

const props = defineProps<{
  diff: string
  path?: string
  title?: string
  language?: Lang
}>()

const t = computed(() => strings(props.language))

interface HighlightedRow extends DiffRow {
  spans: HighlightSpan[]
}

interface HighlightedHunk {
  header: string
  rows: HighlightedRow[]
  pairs: { left?: HighlightedRow; right?: HighlightedRow }[]
}

function enrichRow(row: DiffRow | undefined, path: string | undefined): HighlightedRow | undefined {
  if (!row) {
    return undefined
  }

  const clean = safeText(row.text)
  return {
    ...row,
    text: clean,
    spans: highlightSingleLine(clean, path),
  }
}

const hunks = computed<HighlightedHunk[]>(() =>
  parseUnifiedDiff(props.diff).map((hunk) => ({
    header: safeText(hunk.header),
    rows: hunk.rows.map((row) => enrichRow(row, props.path)!),
    pairs: hunk.pairs.map((pair) => ({
      left: enrichRow(pair.left, props.path),
      right: enrichRow(pair.right, props.path),
    })),
  })),
)

function rowBg(kind: DiffRow['kind'] | undefined): string {
  if (kind === 'del') {
    return 'bg-red-500/10 dark:bg-red-500/15'
  }

  if (kind === 'add') {
    return 'bg-emerald-500/10 dark:bg-emerald-500/15'
  }

  return ''
}

function prefixFor(kind: DiffRow['kind'] | undefined): string {
  if (kind === 'del') {
    return '-'
  }

  if (kind === 'add') {
    return '+'
  }

  return ' '
}
</script>

<template>
  <div v-if="hunks.length" class="overflow-hidden rounded-lg border bg-card">
    <div
      v-if="title"
      class="flex items-center justify-between gap-2 border-b bg-muted/50 px-3 py-1.5 text-xs font-medium text-muted-foreground"
    >
      <span>{{ title }}</span>
      <span v-if="path" class="font-mono break-all">{{ safeText(path) }}</span>
    </div>

    <!-- Side-by-side on wide viewports -->
    <div class="hidden overflow-x-auto lg:block print:hidden">
      <table class="w-full table-fixed border-collapse font-mono text-xs leading-relaxed">
        <colgroup>
          <col class="w-12" />
          <col class="w-5" />
          <col class="w-[calc(50%-4.25rem)]" />
          <col class="w-12" />
          <col class="w-5" />
          <col class="w-[calc(50%-4.25rem)]" />
        </colgroup>
        <template v-for="(hunk, hIdx) in hunks" :key="hIdx">
          <tbody>
            <tr class="border-y bg-muted/40 text-muted-foreground">
              <td colspan="6" class="px-3 py-1 text-[11px]">{{ hunk.header }}</td>
            </tr>
            <tr v-for="(pair, pIdx) in hunk.pairs" :key="pIdx">
              <td
                class="pr-2 text-right text-muted-foreground tabular-nums select-none"
                :class="pair.left ? rowBg(pair.left.kind) : 'bg-muted/20'"
              >
                {{ pair.left?.oldLine ?? '' }}
              </td>
              <td
                class="text-center select-none"
                :class="[
                  pair.left ? rowBg(pair.left.kind) : 'bg-muted/20',
                  pair.left?.kind === 'del'
                    ? 'text-red-700 dark:text-red-300'
                    : 'text-muted-foreground',
                ]"
              >
                {{ pair.left ? prefixFor(pair.left.kind) : '' }}
              </td>
              <td
                class="overflow-x-auto border-r pr-3 whitespace-pre"
                :class="pair.left ? rowBg(pair.left.kind) : 'bg-muted/20'"
              >
                <template v-if="pair.left">
                  <template v-for="(span, sIdx) in pair.left.spans" :key="sIdx">
                    <span :class="span.kind ? `sych-tok-${span.kind}` : undefined">{{
                      span.text
                    }}</span>
                  </template>
                  <span
                    v-if="pair.left.noNewline"
                    class="ml-2 rounded bg-muted px-1 text-[10px] text-muted-foreground"
                  >
                    {{ t.noEofNewline }}
                  </span>
                </template>
              </td>

              <td
                class="pr-2 text-right text-muted-foreground tabular-nums select-none"
                :class="pair.right ? rowBg(pair.right.kind) : 'bg-muted/20'"
              >
                {{ pair.right?.newLine ?? '' }}
              </td>
              <td
                class="text-center select-none"
                :class="[
                  pair.right ? rowBg(pair.right.kind) : 'bg-muted/20',
                  pair.right?.kind === 'add'
                    ? 'text-emerald-700 dark:text-emerald-300'
                    : 'text-muted-foreground',
                ]"
              >
                {{ pair.right ? prefixFor(pair.right.kind) : '' }}
              </td>
              <td
                class="overflow-x-auto pr-3 whitespace-pre"
                :class="pair.right ? rowBg(pair.right.kind) : 'bg-muted/20'"
              >
                <template v-if="pair.right">
                  <template v-for="(span, sIdx) in pair.right.spans" :key="sIdx">
                    <span :class="span.kind ? `sych-tok-${span.kind}` : undefined">{{
                      span.text
                    }}</span>
                  </template>
                  <span
                    v-if="pair.right.noNewline"
                    class="ml-2 rounded bg-muted px-1 text-[10px] text-muted-foreground"
                  >
                    {{ t.noEofNewline }}
                  </span>
                </template>
              </td>
            </tr>
          </tbody>
        </template>
      </table>
    </div>

    <!-- Unified diff on compact viewports and print -->
    <div class="overflow-x-auto lg:hidden print:block">
      <table class="w-full border-collapse font-mono text-xs leading-relaxed">
        <template v-for="(hunk, hIdx) in hunks" :key="hIdx">
          <tbody>
            <tr class="border-y bg-muted/40 text-muted-foreground">
              <td colspan="4" class="px-3 py-1 text-[11px]">{{ hunk.header }}</td>
            </tr>
            <tr v-for="(row, rIdx) in hunk.rows" :key="rIdx" :class="rowBg(row.kind)">
              <td class="w-11 pr-2 text-right text-muted-foreground tabular-nums select-none">
                {{ row.oldLine ?? '' }}
              </td>
              <td class="w-11 pr-2 text-right text-muted-foreground tabular-nums select-none">
                {{ row.newLine ?? '' }}
              </td>
              <td
                class="w-5 text-center select-none"
                :class="
                  row.kind === 'del'
                    ? 'text-red-700 dark:text-red-300'
                    : row.kind === 'add'
                      ? 'text-emerald-700 dark:text-emerald-300'
                      : 'text-muted-foreground'
                "
              >
                {{ prefixFor(row.kind) }}
              </td>
              <td class="pr-4 whitespace-pre">
                <template v-for="(span, sIdx) in row.spans" :key="sIdx">
                  <span :class="span.kind ? `sych-tok-${span.kind}` : undefined">{{
                    span.text
                  }}</span>
                </template>
                <span
                  v-if="row.noNewline"
                  class="ml-2 rounded bg-muted px-1 text-[10px] text-muted-foreground"
                >
                  {{ t.noEofNewline }}
                </span>
              </td>
            </tr>
          </tbody>
        </template>
      </table>
    </div>
  </div>
</template>
