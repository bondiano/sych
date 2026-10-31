import { fileURLToPath } from 'node:url'

import { configFromProject } from '@o-feod/eslint-structure-plugin'
import pluginVitest from '@vitest/eslint-plugin'
import { defineConfigWithVueTs, vueTsConfigs } from '@vue/eslint-config-typescript'
import skipFormatting from 'eslint-config-prettier/flat'
import { globalIgnores } from 'eslint/config'
import pluginOxlint from 'eslint-plugin-oxlint'
import pluginVue from 'eslint-plugin-vue'

export default defineConfigWithVueTs(
  { name: 'app/files-to-lint', files: ['**/*.{vue,ts,mts}'] },

  globalIgnores(['**/dist/**', '**/coverage/**', 'src/modules/Api/schema.gen.d.ts']),

  ...pluginVue.configs['flat/recommended'],
  vueTsConfigs.recommendedTypeChecked,

  // FEOD layers: app → pages → modules → common; a module is used through its index only.
  {
    ...configFromProject({
      rootDir: fileURLToPath(new URL('.', import.meta.url)),
      preset: 'strict',
    }),
    name: 'app/feod-architecture',
    files: ['src/**/*.{ts,vue}'],
  },

  // FEOD names pages by their URL (`index.vue`, `[...404].vue`).
  {
    files: ['src/pages/**/*.vue', 'src/app/layouts/*.vue'],
    rules: { 'vue/multi-word-component-names': 'off' },
  },

  // shadcn-vue's generated kit keeps its own names and optional props as it ships them.
  {
    files: ['src/modules/Ui/components/**/*.vue'],
    rules: { 'vue/multi-word-component-names': 'off', 'vue/require-default-prop': 'off' },
  },

  { ...pluginVitest.configs.recommended, files: ['src/**/__tests__/*', 'src/**/*.spec.ts'] },

  ...pluginOxlint.buildFromOxlintConfigFile('.oxlintrc.json'),

  skipFormatting,
)
