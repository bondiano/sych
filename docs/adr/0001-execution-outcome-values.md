# ADR 0001. `ExecutionOutcome` values in ReviewResult

**Status:** accepted, October 25, 2026. **Affects:** SYCH_SPEC v1.2 §5.7, §8.1–8.2; roadmap M0.1, M0.4; INV-02, INV-03.

## Context

§5.7 requires an `execution_outcome` field in every ReviewResult but does not list its values.
`ExecutionState` from §8.1 does not fit: the result is saved only on the transition
`FINALIZING → COMPLETED`, so inside an immutable result the state would always be `COMPLETED`
and would not tell how the run reached finalization. Coverage and recommendation are separate dimensions (INV-02)
and do not replace this field either.

## Decision

`execution_outcome` describes by which §8.1 transition the run entered FINALIZING:

| Value | §8.1 transition / §8.2 case | Meaning |
|---|---|---|
| `PLAN_EXECUTED` | RUNNING → FINALIZING, plan completed | All planned tasks were executed |
| `LIMIT_REACHED` | RUNNING → FINALIZING, limit reached | A budget/size limit stopped the plan; what was skipped is listed as gaps |
| `RETRIES_EXHAUSTED` | RETRY_WAIT → FINALIZING | Retries were exhausted; an honest partial result was saved |
| `INPUT_UNAVAILABLE` | §8.2, diff not obtained | Substantive analysis is impossible; coverage `UNAVAILABLE` |

`CANCELLED`, `SUPERSEDED` and `FAILED` are not values: no ReviewResult is saved in these states.

## Consequences

- The value does not affect the recommendation by itself: the reducer computes it from obligations and findings (§5.7).
  `PLAN_EXECUTED` does not mean `COMPLETE_FOR_SCOPE`, and `LIMIT_REACHED`/`RETRIES_EXHAUSTED` do not allow it (M0.4).
- The source of truth is `sych_core::ExecutionOutcome`; the JSON Schema is generated from it.
  Adding a value is an enum change under §25.4: it needs a compatibility path and a test of an old fixture.
- In the next revision of the specification, §5.7 should reference this table.
