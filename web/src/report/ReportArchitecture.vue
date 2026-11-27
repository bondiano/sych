<script setup lang="ts">
import { computed } from 'vue'

import type { Schemas } from '@/modules/Api'
import {
  CodePlace,
  ProseText,
  safeText,
  strings,
  type AnyCodeLinks,
  type Lang,
} from '@/modules/Runs'
import { StatusBadge } from '@/modules/Ui'

const props = defineProps<{
  projects: Schemas['ArchitectureFacts'][]
  findings: Schemas['ReportFinding'][]
  links?: AnyCodeLinks
  language?: Lang
}>()

const t = computed(() => strings(props.language))

const standalonePictures = computed(() => {
  if (props.projects.length > 0) {
    return []
  }

  return props.findings.filter((f) => Boolean(f.picture))
})

const hasArchitecture = computed(
  () => props.projects.length > 0 || standalonePictures.value.length > 0,
)
</script>

<template>
  <section v-if="hasArchitecture" class="space-y-4">
    <h2 class="text-lg font-semibold">{{ t.architectureHeading }}</h2>

    <div v-if="projects.length" class="divide-y rounded-lg border bg-card">
      <div
        v-for="project in projects"
        :key="`${project.language}:${project.project}`"
        class="space-y-3 p-5 text-sm"
      >
        <div class="flex flex-wrap items-center gap-2">
          <span class="font-medium capitalize">{{ safeText(project.language) }}</span>
          <span class="font-mono text-xs text-muted-foreground">
            {{ safeText(project.project || '.') }}
          </span>
          <StatusBadge :value="project.status" />
          <span class="text-xs text-muted-foreground">
            {{ project.base_checked ? t.baseChecked : t.baseNotChecked }}
          </span>
        </div>

        <ul v-if="project.coverage.some((c) => c.reason)" class="space-y-1 text-xs">
          <li
            v-for="cov in project.coverage.filter((c) => c.reason)"
            :key="cov.check"
            class="text-muted-foreground"
          >
            <span class="font-mono text-foreground">{{ cov.check }}</span>
            ({{ cov.completeness }}): <ProseText :text="cov.reason ?? ''" />
          </li>
        </ul>

        <div v-if="project.findings.length" class="space-y-3">
          <div
            v-for="(item, idx) in project.findings"
            :key="`${item.check}:${item.key}:${idx}`"
            class="space-y-2 rounded-md border bg-muted/30 p-3"
          >
            <div class="flex flex-wrap items-center gap-2 text-xs">
              <StatusBadge :value="item.change ?? 'UNKNOWN'" />
              <span class="font-mono font-medium">{{ item.check }}</span>
              <CodePlace v-if="item.code_ref" :code="item.code_ref" :links="links" />
            </div>
            <p><ProseText :text="item.message" /></p>
            <pre
              v-if="item.picture.length"
              class="overflow-x-auto rounded border bg-card p-2.5 font-mono text-xs leading-snug"
              >{{ safeText(item.picture.join('\n')) }}</pre>
          </div>
        </div>

        <div v-if="project.fixed?.length" class="space-y-1 text-xs text-muted-foreground">
          <p class="font-medium text-foreground">{{ t.fixedInHead }}</p>
          <ul class="space-y-0.5">
            <li v-for="fixed in project.fixed" :key="`${fixed.check}:${fixed.key}`">
              <span class="font-mono">{{ fixed.check }}</span
              >: <ProseText :text="fixed.message" />
            </li>
          </ul>
        </div>

        <ul v-if="project.limitations?.length" class="space-y-1 text-xs text-muted-foreground">
          <li v-for="lim in project.limitations" :key="lim">
            <span class="font-medium text-foreground">{{ t.limitation }}</span
            >{{ ' ' }}<ProseText :text="lim" />
          </li>
        </ul>
      </div>
    </div>

    <div v-else class="divide-y rounded-lg border bg-card">
      <div v-for="f in standalonePictures" :key="f.id" class="space-y-2 p-5 text-sm">
        <div class="flex flex-wrap items-center gap-2">
          <span v-if="f.rule_id" class="font-mono text-xs text-muted-foreground">
            {{ safeText(f.rule_id) }}
          </span>
          <a
            :href="`#finding-${f.id}`"
            class="font-medium underline decoration-muted-foreground/40 underline-offset-2 hover:decoration-current"
          >
            <ProseText :text="f.title" />
          </a>
        </div>
        <pre
          class="overflow-x-auto rounded-md border bg-muted/40 p-3 font-mono text-xs leading-snug"
          >{{ safeText(f.picture ?? '') }}</pre>
      </div>
    </div>
  </section>
</template>
