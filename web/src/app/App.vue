<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'

import DefaultLayout from './layouts/default.vue'

const route = useRoute()
const bare = computed(() => route.meta.public === true)
</script>

<template>
  <RouterView v-if="bare" />
  <DefaultLayout v-else>
    <RouterView />
  </DefaultLayout>
</template>
