import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'

import type { Schemas } from '@/modules/Api'

import ReportApp from '../ReportApp.vue'

function sampleView(lang: Schemas['ReviewLanguage'] = 'en'): Schemas['ReportView'] {
  const sha = 'sha1:' + '1'.repeat(40)
  const snapId = 'sha256:' + '2'.repeat(64)
  const digest = 'sha256:' + '3'.repeat(64)
  return {
    schema_version: 1,
    header: {
      run_id: '0192f000-0000-7000-8000-000000000010',
      result_id: '0192f000-0000-7000-8000-000000000011',
      review_revision: 2,
      snapshot: {
        schema_version: 1,
        id: snapId,
        repository: 'local:demo',
        target_ref: 'main',
        target_tip: sha,
        diff_base: sha,
        head: sha,
        head_ref: 'feat/demo',
        head_kind: 'STAGED',
        comparison: 'MERGE_BASE',
        source_manifest_hash: digest,
        diff_manifest_hash: digest,
        created_at: '2026-11-28T00:00:00Z',
      },
      recommendation: 'ATTENTION_REQUIRED',
      coverage: 'PARTIAL',
      execution_outcome: 'PLAN_EXECUTED',
      counts: [
        { severity: 'HIGH', category: 'DEFECT', count: 1 },
        { severity: 'LOW', category: 'NIT', count: 1 },
      ],
      usage: {
        model_attempts: 2,
        input_tokens: 1200,
        output_tokens: 300,
        reasoning_tokens: 0,
        unknown_usage_attempts: 0,
        cost: { known: 4500, reference_estimate: 4500, unknown_cost_attempts: 0 },
        routes: [],
      },
      language: lang,
      created_at: '2026-11-28T00:00:00Z',
      provenance: { engine_version: '0.2.0', model_adapters: ['fake'] },
      scope: {
        revision: 1,
        units: [
          {
            id: '0192f000-0000-7000-8000-000000000012',
            title: 'orders',
            paths: ['src/orders.rs'],
          },
        ],
        required_obligation_ids: [],
        requested_profiles: ['correctness'],
        excluded: [],
      },
    },
    findings: [
      {
        id: '0192f000-0000-7000-8000-000000000013',
        category: 'DEFECT',
        severity: 'HIGH',
        lifecycle: 'OPEN',
        verification_status: 'CONFIRMED',
        support: 'STATIC_TRACE',
        title: 'Unchecked order total',
        explanation: 'Overflow wraps silently.',
        picture: 'orders ──> billing',
        triggering_conditions: ['when quantity is large'],
        consequence: 'undercharged order',
        anchors: [
          {
            reference: {
              snapshot_id: snapId,
              side: 'HEAD',
              path: 'src/orders.rs',
              blob_oid: sha,
              start_line: 2,
              end_line: 2,
              content_sha256: digest,
            },
            excerpt: {
              first_line: 1,
              text: 'pub fn total(q: u32, p: u32) -> u32 {\n    q * p\n}\n',
            },
            diff: '--- a/src/orders.rs\n+++ b/src/orders.rs\n@@ -1,3 +1,3 @@\n pub fn total(q: u32, p: u32) -> u32 {\n-    q.saturating_mul(p)\n+    q * p\n }\n',
          },
        ],
        also: [],
        suggested_action: 'Use checked_mul.',
        validation_expectation: 'Overflow returns an error.',
        limitations: [],
        rule_id: null,
        remediation: null,
        suggested_patch:
          'diff --git a/src/orders.rs b/src/orders.rs\n--- a/src/orders.rs\n+++ b/src/orders.rs\n@@ -1,3 +1,3 @@\n pub fn total(q: u32, p: u32) -> u32 {\n-    q * p\n+    q.saturating_mul(p)\n }\n',
        fix_note: null,
      },
      {
        id: '0192f000-0000-7000-8000-000000000014',
        category: 'NIT',
        severity: 'LOW',
        lifecycle: 'OPEN',
        verification_status: 'CONFIRMED',
        support: 'PREFERENCE',
        title: 'Short parameter name',
        explanation: 'Prefer `price`.',
        triggering_conditions: [],
        consequence: '',
        anchors: [
          {
            reference: {
              snapshot_id: snapId,
              side: 'HEAD',
              path: 'src/other.rs',
              blob_oid: sha,
              start_line: 1,
              end_line: 1,
              content_sha256: digest,
            },
            excerpt: null,
            diff: null,
          },
        ],
        also: [],
        suggested_action: 'Rename `p` to `price`.',
        validation_expectation: '',
        limitations: [],
        rule_id: null,
        remediation: null,
        suggested_patch: null,
        fix_note: null,
      },
    ],
    gaps: {
      unknown_obligations: [
        {
          id: '0192f000-0000-7000-8000-000000000015',
          rule_id: 'EFFECT-01',
          question: 'Event committed with order',
          required: true,
          reason_code: 'NOT_ASSESSED',
          next_evidence_needed: 'model pass',
        },
      ],
      paths: [],
      open_questions: [],
      unresolved_candidates: [],
    },
    fixes_patch:
      'diff --git a/src/orders.rs b/src/orders.rs\n--- a/src/orders.rs\n+++ b/src/orders.rs\n@@ -1,3 +1,3 @@\n pub fn total(q: u32, p: u32) -> u32 {\n-    q * p\n+    q.saturating_mul(p)\n }\n',
    obligations: [
      {
        id: '0192f000-0000-7000-8000-000000000015',
        rule_id: 'EFFECT-01',
        rule_version: 1,
        question: 'Event committed with order',
        required: true,
        risk_level: 'HIGH',
        review_unit_ids: [],
        code_refs: [],
        applicability: 'APPLIES',
        acceptance_criterion: 'Committed together',
        acceptable_evidence_kinds: ['source'],
        status: 'UNKNOWN',
        reason_code: 'NOT_ASSESSED',
        next_evidence_needed: 'model pass',
        evidence_ids: [],
        limitations: [],
      },
    ],
  }
}

describe('ReportApp', () => {
  it('renders verdict, fix-first list, grouped files, gaps, architecture and collapsed sections', async () => {
    const wrapper = mount(ReportApp, { props: { view: sampleView('en') } })
    expect(wrapper.text()).toContain('1 problem, 1 high or critical')
    expect(wrapper.text()).toContain('staged index (uncommitted)')
    expect(wrapper.text()).toContain('Fix first')
    expect(wrapper.text()).toContain('src/orders.rs')
    expect(wrapper.text()).toContain('src/other.rs')
    expect(wrapper.text()).toContain('Coverage gaps and unknowns')
    expect(wrapper.text()).toContain('EFFECT-01')
    expect(wrapper.text()).toContain('orders ──> billing')
    expect(wrapper.text()).toContain('Copy patch')

    // Filter by file path
    const input = wrapper.find('input[type="search"]')
    await input.setValue('orders')
    const findingsSection = wrapper.get('[data-section="findings"]').text()
    expect(findingsSection).toContain('Unchecked order total')
    expect(findingsSection).not.toContain('Short parameter name')
  })

  it('follows Russian labels when review.language is ru', () => {
    const wrapper = mount(ReportApp, { props: { view: sampleView('ru') } })
    expect(wrapper.text()).toContain('Ревью Sych')
    expect(wrapper.text()).toContain('Исправить в первую очередь')
    expect(wrapper.text()).toContain('Пробелы покрытия и неизвестные')
    expect(wrapper.text()).toContain('Скопировать патч')
  })
})
