import './styles.css'

import { createApp } from 'vue'

import type { Schemas } from '@/modules/Api'

import ReportApp from './ReportApp.vue'

function loadEmbeddedView(): Schemas['ReportView'] | null {
  const el = document.getElementById('sych-report-data')
  const raw = el?.textContent?.trim()
  if (!raw) {
    return null
  }

  try {
    return JSON.parse(raw) as Schemas['ReportView']
  } catch {
    // Unparseable or placeholder JSON renders the empty error state rather than throwing.
    return null
  }
}

const view = loadEmbeddedView()
if (view?.header.language) {
  document.documentElement.lang = view.header.language
}

createApp(ReportApp, { view }).mount('#app')
