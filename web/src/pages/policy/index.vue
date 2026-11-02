<script setup lang="ts">
import { computed } from 'vue'

import { useQueryParam } from '@/common/composables/useQueryParam'
import { toId } from '@/common/utilities/toId'
import { usePolicy } from '@/modules/Policy'
import { RepositorySelect } from '@/modules/Session'
import { ErrorNotice, Skeleton } from '@/modules/Ui'

const repository = useQueryParam('repository')
const id = computed(() => toId(repository.value))
const policy = usePolicy(id)
const source = computed(() => {
  const s = policy.data.value?.snapshot.source
  return s?.kind === 'trusted_ref' ? `${s.ref_name} @ ${s.commit}` : 'engine defaults'
})
</script>

<template>
  <div class="space-y-6">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="mr-auto text-xl font-semibold">Policy</h1>
      <RepositorySelect v-model="repository" />
    </div>
    <p class="text-sm text-muted-foreground">
      Read only: the policy of the repository's newest run, read again at the commit it pinned.
      Policy changes by a pull request to <span class="font-mono">.sych/</span>.
    </p>
    <p v-if="id === undefined" class="text-sm text-muted-foreground">Choose a repository.</p>
    <ErrorNotice
      v-else-if="policy.error.value"
      :error="policy.error.value"
      title="No policy to show"
    />
    <Skeleton v-else-if="!policy.data.value" class="h-40 w-full" />
    <template v-else>
      <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1 text-sm">
        <dt class="text-muted-foreground">Source</dt>
        <dd class="font-mono break-all">{{ source }}</dd>
        <dt class="text-muted-foreground">Run</dt>
        <dd>
          <RouterLink :to="`/runs/${policy.data.value.run_id}`" class="font-mono underline">
            {{ policy.data.value.run_id }}
          </RouterLink>
        </dd>
        <dt class="text-muted-foreground">Rules hash</dt>
        <dd class="font-mono break-all">{{ policy.data.value.snapshot.rules_hash }}</dd>
        <dt class="text-muted-foreground">Config hash</dt>
        <dd class="font-mono break-all">{{ policy.data.value.snapshot.config_hash }}</dd>
      </dl>
      <p v-if="!policy.data.value.matches_snapshot" class="text-sm text-destructive">
        The server config or Sych changed since that run: the policy read now has other hashes than
        the run pinned.
      </p>
      <section>
        <h2 class="mb-3 text-lg font-medium">Effective policy</h2>
        <pre class="overflow-x-auto rounded-lg border p-4 text-xs">{{
          JSON.stringify(policy.data.value.effective, null, 2)
        }}</pre>
      </section>
    </template>
  </div>
</template>
