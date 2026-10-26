# Sych review

- recommendation: **NO_BLOCKING_FINDINGS_IN_SCOPE**
- coverage: **COMPLETE_FOR_SCOPE**
- execution outcome: PLAN_EXECUTED
- run: 01a17361-17cc-70a2-9ed0-5e4ec7cc6c61 (revision 1), result: 01a17361-9754-7262-80c4-fdf6950bdfde, created 2026-10-25T14:05:05.299625298Z

## Snapshot

- repository: local:sha256:fc09d9164b7a097209bc5cf361defc3b1162bca19ad516039187f599caedac9c
- target: HEAD\~1 at sha1:fc7286d51164bb18a7f618177cb8fbe55c7f9640
- diff base: sha1:fc7286d51164bb18a7f618177cb8fbe55c7f9640 (MERGE_BASE)
- head: sha1:ebd3d57d5e3a96372d1ea79293985a13d767dbaf
- snapshot: sha256:a2f259d50e8df71ef7d71eb38d16cc555cb83f6d32fb9a815e60329c1aa9b85e

## Policy

- source: trusted ref HEAD\~1 at sha1:fc7286d51164bb18a7f618177cb8fbe55c7f9640
- policy snapshot: sha256:238ed92b73d6f4a375c8871cb26cad1f48b88f8234bf3baaa615a3dc28122962
- rules: sha256:4151dc323cbac836a831c9095d323b79c14e63bed1728033e411228ce130a472, config: sha256:ea32f0d72a21b40e2610f1ab25c4ef8d2ef6a21302bd88b4bafc4ed00a043f48

Provenance:

- engine: sych 0.1.0 
- server: sych.yaml sha256:5f81a52006d3892015d70533fb176d1982960a9c4473b71f79721684392a6b55
- repository: .sych/rules.yaml@HEAD\~1@sha1:fc7286d51164bb18a7f618177cb8fbe55c7f9640 sha256:158d0409615b4d0c2a08a6017cae0a28e2d7a2e6b76780e0e9653b7ee1044d98

Proposed by head, not applied:

- none

## Scope (revision 1)

- src: src/order.ts

## Obligations

- **ORDER-01** v1 (required, risk HIGH, applicability APPLIES): SUPPORTED
  - id: c1f15f52-680a-87a9-8787-f7c4eed6d6c8
  - question: total() returns 0 when items is undefined instead of throwing.
  - acceptance: total() returns 0 when items is undefined instead of throwing.
  - evidence: dead7230-e4e2-8bd1-ae74-51cbfa4d5415
  - limitation: sum is not shown, but reduce with an explicit initial value returns that value unchanged for an empty array, so the callback body is irrelevant here.

## Findings

- none

## Gaps

- unverified candidate: HEAD changes behaviour for falsy non-nullish inputs: total(0) (or '', false, NaN) returned 0 in BASE but now throws TypeError because 0 is not nullish and has no reduce.
- unverified candidate: Consider distinguishing null/undefined from other invalid inputs, e.g. Array.isArray(items) ? items.reduce(sum, 0) : 0, so a silent-zero result is only produced for the cases the rule intends.

## Evidence

- 0e8208ee-2dcd-8df4-8eb3-6bb13c74769f semantic UNAVAILABLE UNKNOWN DERIVED_LOCAL by sych-semantic 0\.1.0
  - limitation: unsupported: no pnpm-lock.yaml or package-lock.json at the repository root
- 16e388b5-24a7-8167-b08a-b6573e4c2f07 semantic UNAVAILABLE UNKNOWN DERIVED_LOCAL by sych-semantic 0\.1.0
  - limitation: unsupported: no pnpm-lock.yaml or package-lock.json at the repository root
- a2768306-1110-8f1e-a003-a494fed8f819 source VALID COMPLETE_FOR_QUERY SOURCE_OBSERVATION by sych-broker 0\.1.0
  - src/order.ts:1-6 (DIFF_BASE, blob sha1:1bd578a33e66c60044aae6a965f4e932524e79b9)
  - src/order.ts:1-3 (HEAD, blob sha1:9a587d0ab4d5dbbc4242c40617ee058dddea45cb)
  - limitation: semantic backend Unsupported for src/order.ts (base): unsupported: no pnpm-lock.yaml or package-lock.json at the repository root
  - limitation: semantic backend Unsupported for src/order.ts (head): unsupported: no pnpm-lock.yaml or package-lock.json at the repository root
- da17a96d-02ad-8f1b-9d4e-3deeaa028854 static VALID PARTIAL DERIVED_LOCAL by oxc 0\.139
  - limitation: macros are not expanded and types are not resolved: facts are shapes, and a missing fact is not proof of absence
- dead7230-e4e2-8bd1-ae74-51cbfa4d5415 source VALID COMPLETE_FOR_QUERY SOURCE_OBSERVATION by sych-broker 0\.1.0
  - src/order.ts:1-6 (DIFF_BASE, blob sha1:1bd578a33e66c60044aae6a965f4e932524e79b9)
  - src/order.ts:1-3 (HEAD, blob sha1:9a587d0ab4d5dbbc4242c40617ee058dddea45cb)
  - limitation: semantic backend Unsupported for src/order.ts (base): unsupported: no pnpm-lock.yaml or package-lock.json at the repository root
  - limitation: semantic backend Unsupported for src/order.ts (head): unsupported: no pnpm-lock.yaml or package-lock.json at the repository root
- ee36010d-3cac-8d93-828b-2183da162f46 static VALID PARTIAL DERIVED_LOCAL by oxc 0\.139
  - limitation: macros are not expanded and types are not resolved: facts are shapes, and a missing fact is not proof of absence

## Usage

- model attempts: 3, tokens in/out: 3437/3094 (reasoning 1748 of output), attempts with unreported tokens: 0
- known cost: 1521 micro-USD, attempts with unknown cost: 0, list-price estimate: 4745 micro-USD
- route cheap\_api:deepseek/deepseek-v4.1-flash: api_usage, api_hard, 3 attempts
- engine 0\.1.0, model adapters: cheap\_api:deepseek/deepseek-v4.1-flash
