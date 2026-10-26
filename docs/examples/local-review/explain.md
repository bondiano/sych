# Obligation EFFECT-01

- **EFFECT-01** v1 (required, risk HIGH, applicability APPLIES): VIOLATED
  - id: 532b88b5-3538-8f87-91df-815acea119c2
  - question: A stored order publishes order-created in the same transaction.
  - acceptance: Commit boundary and delivery mechanism are identified.
  - evidence: cd0c5126-7e1b-897c-8ca5-13c24a626500

## Evidence

- cd0c5126-7e1b-897c-8ca5-13c24a626500 source VALID COMPLETE_FOR_QUERY SOURCE_OBSERVATION by fake 0\.1.0
  - backend/orders/store.go:4-8 (HEAD, blob sha1:c47cd2331554fdc05de3fdb760ffee67a9f0c90c)

## Context

- PARTIAL · 238 tokens of 6000 · 2 files · 0 semantic items
  - REQUIRED `backend/orders/store.go:4-9` (DIFF_BASE): diff hunk (base)
  - REQUIRED `backend/orders/store.go:4-8` (HEAD): diff hunk (head)
  - gap: NO\_SEMANTIC\_BACKEND: backend/orders/store.go (base, .go)
  - gap: NO\_SEMANTIC\_BACKEND: backend/orders/store.go (head, .go)

## Findings

- 8520041a-d476-8e81-a455-8c3a5f2f6a14 HIGH: EFFECT-01 violated: Stored order publishes order-created

