# Sych review

- recommendation: **ATTENTION_REQUIRED**
- coverage: **COMPLETE_FOR_SCOPE**
- execution outcome: PLAN_EXECUTED
- run: 01a17360-b60c-72d9-ad31-b2b960fd525f (revision 1), result: 01a17361-164e-76d7-b28b-36c541655244, created 2026-10-25T14:04:40.238978572Z

## Snapshot

- repository: local:sha256:e68565c4bf8fb73aa7a4b309344fcdc41cd904e9b02345d925d463d48148d7f5
- target: HEAD\~1 at sha1:fc7286d51164bb18a7f618177cb8fbe55c7f9640
- diff base: sha1:fc7286d51164bb18a7f618177cb8fbe55c7f9640 (MERGE_BASE)
- head: sha1:cd38ac682a78df64616c4cce6c47f08e904ddc80
- snapshot: sha256:528c6dde2b34ca8ec4f13fd16de042554c85677f72e7b74c2ca500cf175fef8f

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

- **ORDER-01** v1 (required, risk HIGH, applicability APPLIES): VIOLATED
  - id: 40265917-1a5b-8906-8d27-12ceee764e5a
  - question: total() returns 0 when items is undefined instead of throwing.
  - acceptance: total() returns 0 when items is undefined instead of throwing.
  - evidence: b70c6bbd-0ac7-86cd-ac0c-0cea226a666b
  - limitation: No runtime execution was performed; the throw is inferred from the removed guard and the non-null assertion, which is erased at runtime.

## Findings

- **HIGH** DEFECT OPEN: Removed falsy-items guard makes total(undefined) throw instead of returning 0
  - BASE returned 0 for falsy items; HEAD calls items!.reduce with no runtime guard, and the non-null assertion is erased, so undefined reaches reduce and throws. This contradicts the stated rule that total() returns 0 for undefined items.
  - consequence: Missing items now crash callers that previously received 0, a behavioural regression directly contradicting ORDER-01.
  - at src/order.ts:2-2 (HEAD, blob sha1:0d63ee2a8ff45ab02c4c92345f52e0ad26975282)
  - at src/order.ts:2-4 (DIFF_BASE, blob sha1:1bd578a33e66c60044aae6a965f4e932524e79b9)
  - at .sych/rules.yaml:12-12 (HEAD, blob sha1:b52d61c2602e8c6f4084ea9d627cb143b3744867)
  - action: Restore a runtime falsy check or default parameter before calling reduce.

## Gaps

- none

## Evidence

- 1831bd8f-0c0c-89e7-8348-6a241b8bb7a0 semantic UNAVAILABLE UNKNOWN DERIVED_LOCAL by sych-semantic 0\.1.0
  - limitation: unsupported: no pnpm-lock.yaml or package-lock.json at the repository root
- afd8f120-f592-8a96-8e39-eeaa52a48248 static VALID PARTIAL DERIVED_LOCAL by oxc 0\.139
  - limitation: macros are not expanded and types are not resolved: facts are shapes, and a missing fact is not proof of absence
- b70c6bbd-0ac7-86cd-ac0c-0cea226a666b source VALID COMPLETE_FOR_QUERY SOURCE_OBSERVATION by sych-broker 0\.1.0
  - src/order.ts:1-6 (DIFF_BASE, blob sha1:1bd578a33e66c60044aae6a965f4e932524e79b9)
  - src/order.ts:1-3 (HEAD, blob sha1:0d63ee2a8ff45ab02c4c92345f52e0ad26975282)
  - limitation: semantic backend Unsupported for src/order.ts (base): unsupported: no pnpm-lock.yaml or package-lock.json at the repository root
  - limitation: semantic backend Unsupported for src/order.ts (head): unsupported: no pnpm-lock.yaml or package-lock.json at the repository root
- c29275a8-d48b-85c1-90fc-96f727552c7d static VALID PARTIAL DERIVED_LOCAL by oxc 0\.139
  - limitation: macros are not expanded and types are not resolved: facts are shapes, and a missing fact is not proof of absence
- c69f15f1-09bc-8582-990b-78fe95222ae3 semantic UNAVAILABLE UNKNOWN DERIVED_LOCAL by sych-semantic 0\.1.0
  - limitation: unsupported: no pnpm-lock.yaml or package-lock.json at the repository root
- cca61dda-c84a-819c-8b80-2383abd06cf8 source VALID COMPLETE_FOR_QUERY SOURCE_OBSERVATION by sych-broker 0\.1.0
  - src/order.ts:1-6 (DIFF_BASE, blob sha1:1bd578a33e66c60044aae6a965f4e932524e79b9)
  - src/order.ts:1-3 (HEAD, blob sha1:0d63ee2a8ff45ab02c4c92345f52e0ad26975282)
  - limitation: semantic backend Unsupported for src/order.ts (base): unsupported: no pnpm-lock.yaml or package-lock.json at the repository root
  - limitation: semantic backend Unsupported for src/order.ts (head): unsupported: no pnpm-lock.yaml or package-lock.json at the repository root

## Usage

- model attempts: 3, tokens in/out: 3351/2636 (reasoning 893 of output), attempts with unreported tokens: 0
- known cost: 1525 micro-USD, attempts with unknown cost: 0, list-price estimate: 4170 micro-USD
- route cheap\_api:deepseek/deepseek-v4.1-flash: api_usage, api_hard, 3 attempts
- engine 0\.1.0, model adapters: cheap\_api:deepseek/deepseek-v4.1-flash
