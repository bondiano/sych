# Sych review

- recommendation: **ATTENTION_REQUIRED**
- coverage: **COMPLETE_FOR_SCOPE**
- execution outcome: PLAN_EXECUTED
- run: 01a172cf-dc7c-758c-8d28-d85bf17678d9 (revision 1), result: 01a172cf-dc9f-76f3-b799-6ed82da3b39e, created 2026-10-25T11:26:27.442282908Z

## Snapshot

- repository: local:sha256:901cde476956e75e1555775e4c9a433ce7eb8becb0ab0ec2c66fd7dbaf8d174b
- target: target at sha1:f15454d20a418344546e0d07ab3768cf7a5fe865
- diff base: sha1:f15454d20a418344546e0d07ab3768cf7a5fe865 (MERGE_BASE)
- head: sha1:8b1d68bd94ad62e63f63cdf2fbafc78fe74ba4b0
- snapshot: sha256:2b886d8f337b3db5142d69332c63d1c8be528dd438a33decdb23903bd2da304e

## Policy

- source: trusted ref target at sha1:f15454d20a418344546e0d07ab3768cf7a5fe865
- policy snapshot: sha256:6d0d21591c665bb5768c6a0339ffcbf2f0f92d094be48801fd0c73b6b67bd864
- rules: sha256:6b07badb9a29059cdbe2c41abd62b6345d090df669351c3061f4787a73c3c9ca, config: sha256:01e119420d0c6dfe9f942c9366d8b1c28007d7f726ca01c70e6c84fa17de0324

Provenance:

- engine: sych 0.1.0 
- repository: .sych/rules.yaml@target@sha1:f15454d20a418344546e0d07ab3768cf7a5fe865 sha256:bc303288aa0e8e71f6d9966a3a35a08cddecd5132574881388968395e4101ef6

Proposed by head, not applied:

- none

## Scope (revision 1)

- backend/orders: backend/orders/store.go

## Obligations

- **EFFECT-01** v1 (required, risk HIGH, applicability APPLIES): VIOLATED
  - id: 532b88b5-3538-8f87-91df-815acea119c2
  - question: A stored order publishes order-created in the same transaction.
  - acceptance: Commit boundary and delivery mechanism are identified.
  - evidence: cd0c5126-7e1b-897c-8ca5-13c24a626500

## Findings

- **HIGH** RULE_VIOLATION OPEN: EFFECT-01 violated: Stored order publishes order-created
  - A stored order publishes order-created in the same transaction.
  - consequence: acceptance criterion not met: Commit boundary and delivery mechanism are identified.
  - at backend/orders/store.go:4-8 (HEAD, blob sha1:c47cd2331554fdc05de3fdb760ffee67a9f0c90c)
  - action: Restore the behavior the rule requires.

## Gaps

- none

## Evidence

- cd0c5126-7e1b-897c-8ca5-13c24a626500 source VALID COMPLETE_FOR_QUERY SOURCE_OBSERVATION by fake 0\.1.0
  - backend/orders/store.go:4-8 (HEAD, blob sha1:c47cd2331554fdc05de3fdb760ffee67a9f0c90c)

## Usage

- model attempts: 0, tokens in/out: 0/0
- known cost: 0 micro-USD, attempts with unknown cost: 0
- engine 0\.1.0, model adapters: fake
