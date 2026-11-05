# ADR 0002. A rejection names its reason; the engine records it and does not check it yet

**Status:** accepted, November 5, 2026. **Affects:** the `verify@7` reply (`CandidateVerdict.reason`), the engine's
handling of a `REJECTED` verdict (`sych_core::analysis::validate_verdicts`) and `VerdictRecord.reason` in a candidate's
verdict history.

## Context

The engine checked a `CONFIRMED` verdict's references and basis, but took `REJECTED` as given, and only `UNRESOLVED`
candidates get the second round with context. The plan was to require, for a defect, a reason and the code a rejection
rests on, and to send a rejection without it to the second round.

The measurement came first. In three runs of the development suite (134 cases, DeepSeek V4.1 Flash) no labelled
defect was lost to a rejection: every verifier loss was a candidate left `UNRESOLVED` (two or three per run), and on
the holdout every miss was one discovery never found. Requiring a basis would have bought no recall and cost second
rounds and more unverified candidates in `unresolved_questions`.

## Decision

`CandidateVerdict.reason` is one of `REFUTED`, `UNREACHABLE`, `NOT_INTRODUCED`, `INTENDED`, or null, and is kept in the
candidate's verdict history. The engine does not check it. A rejection is held only to the reference contract a
confirmation has: one with an unrecognized reference is `UNRESOLVED`. "Not enough evidence to decide" is not a reason
but `UNRESOLVED`.

## Consequences

- A replay of frozen candidates (`eval measure --replay`) shows, verdict by verdict, what each rejection rested on, so
  a later measurement can tell whether rejections lose defects; requiring a basis is the change to make then.
- The values are fixed. Adding one is an enum change with a compatibility path and a test of an old fixture, as for
  [ADR 0001](0001-execution-outcome-values.md). The field is additive: records written before it read as `null`.
