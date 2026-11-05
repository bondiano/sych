# ADR 0002. A rejection names its reason and the code it rests on

**Status:** accepted, November 5, 2026. **Affects:** the `verify@7` reply (`CandidateVerdict.reason`), the engine's
check of a `REJECTED` verdict (`sych_core::analysis::validate_verdicts`) and `VerdictRecord.reason` in a candidate's
verdict history.

## Context

The engine checked a `CONFIRMED` verdict's references and basis, but accepted `REJECTED` as given: a rejection with
references that do not resolve, or with nothing shown at all, ended the candidate. Only `UNRESOLVED` candidates get the
second round with context, so a hypothesis rejected for want of evidence never got it. In the development suite most
missed defects were candidates that reached the labelled lines and that the verifier rejected or left unresolved.

## Decision

`CandidateVerdict.reason` is one of `REFUTED`, `UNREACHABLE`, `NOT_INTRODUCED`, `INTENDED`, or null. For a defect or
rule violation (by the candidate's category or the verdict's), `REJECTED` stands only with a reason and the code the
rejection rests on among its anchors; `NOT_INTRODUCED` needs a BASE anchor. `INTENDED` may go without an anchor when
the names the removed lines lose show the change removing, deleting or adding what they used. A rejection with an
unrecognized reference, without a reason or without that code is `UNRESOLVED` with `REJECTION_WITHOUT_BASIS`, so it
gets the second round. A nit or improvement is still rejected on the verifier's word.

"Not enough evidence to decide" is not a reason: it is `UNRESOLVED`. Duplicates and suppression by policy are decided
by the engine and publication, not named by the model.

## Consequences

- More candidates end `UNRESOLVED` after the first round and are verified again; one still open after the second shows
  in `unresolved_questions` as an unverified candidate.
- The values are fixed. Adding one is an enum change with a compatibility path and a test of an old fixture, as for
  [ADR 0001](0001-execution-outcome-values.md). The field is additive: records written before it read as `null`.
