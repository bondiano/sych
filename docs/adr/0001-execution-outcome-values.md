# ADR 0001. `ExecutionOutcome` values in ReviewResult

**Status:** accepted, October 25, 2026. **Affects:** `ReviewResult.execution_outcome` and the run state machine
(`sych_core::ExecutionState`).

## Context

Every ReviewResult carries an `execution_outcome` field, and its values need a fixed meaning.
The run's `ExecutionState` does not fit: the result is saved only on the transition
`FINALIZING → COMPLETED`, so inside an immutable result the state would always be `COMPLETED`
and would not tell how the run reached finalization. Coverage and recommendation are separate dimensions
and do not replace this field either.

## Decision

`execution_outcome` describes by which run-state transition the run entered FINALIZING:

| Value | Transition | Meaning |
|---|---|---|
| `PLAN_EXECUTED` | RUNNING → FINALIZING, plan completed | All planned tasks were executed |
| `LIMIT_REACHED` | RUNNING → FINALIZING, limit reached | A budget/size limit stopped the plan; what was skipped is listed as gaps |
| `RETRIES_EXHAUSTED` | RETRY_WAIT → FINALIZING | Retries were exhausted; an honest partial result was saved |
| `INPUT_UNAVAILABLE` | diff not obtained | Substantive analysis is impossible; coverage `UNAVAILABLE` |

`CANCELLED`, `SUPERSEDED` and `FAILED` are not values: no ReviewResult is saved in these states.

## Consequences

- The value does not affect the recommendation by itself: the reducer computes it from obligations and findings.
  `PLAN_EXECUTED` does not mean `COMPLETE_FOR_SCOPE`, and `LIMIT_REACHED`/`RETRIES_EXHAUSTED` do not allow it.
- The source of truth is `sych_core::ExecutionOutcome`; the JSON Schema is generated from it.
  Adding a value is an enum change: it needs a compatibility path and a test of an old fixture.
