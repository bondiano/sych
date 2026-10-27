# What senior reviewers write: taxonomy, frequencies, nits

For discovery rules and eval cases. Numbers from the literature and from our sample are kept separate.

## Sources

- Bacchelli, Bird. [Expectations, Outcomes, and Challenges of Modern Code Review](https://sback.it/publications/icse2013.pdf), ICSE 2013 (Microsoft, card sort of 570 comments).
- Czerwonka, Greiler, Tilford. [Code Reviews Do Not Find Bugs](https://www.microsoft.com/en-us/research/wp-content/uploads/2015/05/PID3556473.pdf), ICSE SEIP 2015.
- Bosu, Greiler, Bird. [Characteristics of Useful Code Reviews](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/bosu2015useful.pdf), MSR 2015 (1.5M comments, 5 projects).
- Sadowski et al. [Modern Code Review: A Case Study at Google](https://sback.it/publications/icse2018seip.pdf), ICSE SEIP 2018.
- Mäntylä, Lassenius. What Types of Defects Are Really Discovered in Code Reviews?, IEEE TSE 35(3), 2009, DOI 10.1109/TSE.2008.71 (paywalled; numbers from the [slides](https://slidetodoc.com/what-type-of-defects-are-really-discovered-in/) and Bosu).
- Beller et al. [Modern Code Reviews in OSS: Which Problems Do They Fix?](https://flosshub.org/node/1650), MSR 2014 (from the abstract).
- Google eng-practices: [standard](https://google.github.io/eng-practices/review/reviewer/standard.html), [comments](https://google.github.io/eng-practices/review/reviewer/comments.html).
- [Conventional Comments](https://conventionalcomments.org/).

## Literature: what reviews find

| Source | Measurement |
|---|---|
| Mäntylä 2009 | ~75% of findings are evolvability (documentation, visual representation, structure, organization, solution), functional ~20% (industry) / 13% (students); ratio from 3:1 to 5:1 |
| Beller 2014 | In two OSS projects the same 75:25 maintainability:functional; 7–35% of comments discarded by authors |
| Bacchelli 2013 | 570 comments: code improvement 29% (practices 58, removing unneeded code 55, readability 52), defects 14% (65 of 78 are logic and corner cases) |
| Czerwonka 2015 | ~15% of comments point to a possible defect, ≥50% to maintainability |
| Bosu 2015 | 64–68% useful across all 5 projects; a reviewer who has seen the file before — 65–71% vs 32–37% for a newcomer; usefulness drops with the number of files |
| Sadowski 2018 | Google's expectations: education, maintaining norms, gatekeeping, accident prevention; median 24 lines, 1 reviewer, >80% of changes take at most one iteration |

**Useful vs noise (Bosu, interviews with change authors).** Useful: functional defects, validation and corner cases
(«alternate scenarios where the current implementation may fail»), hints about API/design/team conventions.
Nit-picking (indentation, comments, style, names, typos) is «somewhat useful» or «useful»: «help long-term
project maintenance». Not useful: false positives, questions asked for the reviewer's own understanding, praise, «work for
the future», and remarks on code the change does not touch. Bacchelli: reviewers make formatting remarks «because
they are easy […] but it doesn't really help».

**Google.** «There is no such thing as "perfect" code—there is only better code»; non-critical remarks get the prefix
«Nit:»; «On matters of style, the style guide is the absolute authority», other style is personal preference.
Labels: `Nit:`, `Optional:`, `FYI:`. Conventional Comments: `<label> [decorations]: <subject>`
(`nitpick`, `suggestion`, `issue`, `question`, …; `(non-blocking)`).

## Sample: method

GitHub REST `repos/{o}/{r}/pulls/comments` (newest first; pages 1–3, plus 8–10 for rust, kubernetes,
TypeScript, prometheus to get merged PRs), 25.10.2026. Filter: top-level comment (not a reply), not a bot,
`author_association` MEMBER/COLLABORATOR/OWNER, comment author ≠ PR author, PR **merged**. Result: **428
comments, 197 PRs, 46 reviewers**: prometheus 95, react 87, TypeScript (mostly the Go port) 77, tokio 73, rust
60, kubernetes 36. One primary category per comment, labeled by a single person without an agreement check;
a few reviewers account for a large share (Darksonn, eps1lon, krajorama). A couple of comments explicitly refer to an LLM.

## Sample: frequencies

| Category | n | % |
|---|---|---|
| Tests | 69 | 16.1 |
| Simplification, idioms, reuse, dead code | 58 | 13.6 |
| Documentation, comments, PR description | 51 | 11.9 |
| Correctness: logic, corner case, portability | 40 | 9.3 |
| API/behavior design, types | 39 | 9.1 |
| Question for understanding («Why…?», «What is this for?») | 37 | 8.6 |
| Suggestion only, no text / «Same here» | 33 | 7.7 |
| Process, scope, CI, praise | 30 | 7.0 |
| Performance, allocations | 21 | 4.9 |
| Errors, logging, assert | 18 | 4.2 |
| Naming | 11 | 2.6 |
| Concurrency, resources, lifecycle | 8 | 1.9 |
| Formatting, typos, ordering (linter class) | 7 | 1.6 |
| Consistency with codebase convention | 6 | 1.4 |
| Security | 0 | 0 |

Observations: functional (correctness + errors + concurrency) = 15.4% — matches the 14–15% of Bacchelli and
Czerwonka. Formatting is almost absent (1.6%): rustfmt/gofmt/prettier and CI take care of it. 35 (8.2%) carry the `nit` prefix, and
these are not only style: 8 docs, 5 tests, 5 simplifications, **4 real bugs** (unchecked index, in-place sorting of
an argument). A question mark appears in 42%; ```` ```suggestion ```` in 18%; an identifier in backticks in 56%;
an explicit «not a blocker»/«follow-up» in 2.6%. Median length is 140 characters.

## How a senior writes

1. **Location + mechanism + fix**, one or two sentences. «This bypasses Tokio's coop system. We should decrement the coop
   budget when returning `None`.» The reason is stated through the consequence, not through taste.
2. **Reference to what already exists**: a helper, a neighboring test, a project pattern («like we do for blur», «isn't this a
   duplicate of `wrapParenDurationExpr` above?»).
3. **A question instead of a statement when not certain**, but with a concrete scenario: «`Nano - Scale(exponent)`
   *could* overflow, right?», «Does this work for circular references…?»
4. **Calibration**: «nit:», «not a hard blocker», «probably not called enough to matter», «This is a pre-existing
   issue».
5. **Remove rather than add**: inline a helper, drop a redundant check/else/reset. What reviewers ask to add is a test, a
   why-comment, `#[must_use]`, a `default` in a switch — things that catch a future error.

### Quotes (sample, abridged)

Tests:
- «This test doesn't spawn any threads, so it's useless as a loom test.» [tokio#8505](https://github.com/tokio-rs/tokio/pull/8505#discussion_r4241590214)
- «all of your tests pass on master without your fix» [tokio#8636](https://github.com/tokio-rs/tokio/pull/8636#discussion_r4258997349)
- «we should never silence errors in tests.» [react#37710](https://github.com/react/react/pull/37710#discussion_r4246880960)
- «Let's use Jest's `spyOn` so that 'jest.restoreAllMocks' just works.» [react#37408](https://github.com/react/react/pull/37408#discussion_r3888688799)

Simplification and idioms:
- «This looks like it could be a `matches!`?» [rust#164744](https://github.com/rust-lang/rust/pull/164744#discussion_r4353078944)
- «Can we inline `i32::try_from(backlog).unwrap_or(i32::MAX)` directly … instead of defining a helper function and unit test?» [tokio#8511](https://github.com/tokio-rs/tokio/pull/8511#discussion_r4240972243)
- «Do we need to re-read this again? There's already an `unstable_features` before the loop, right?» [rust#163664](https://github.com/rust-lang/rust/pull/163664#discussion_r4306565573)
- «isn't this a duplicate of `wrapParenDurationExpr` above?» [prometheus#19403](https://github.com/prometheus/prometheus/pull/19403#discussion_r3773888784)
- «nit: let's avoid else here and simply return in the above line» [kubernetes#143232](https://github.com/kubernetes/kubernetes/pull/143232#discussion_r4334588961)

Documentation and comments:
- «The comment above … says this is constant, but I think that's wrong, we do update metadata» [prometheus#20040](https://github.com/prometheus/prometheus/pull/20040#discussion_r4322173701)
- «It makes sense when you know the bug this is fixing, but future readers of this code won't know that.» [tokio#8445](https://github.com/tokio-rs/tokio/pull/8445#discussion_r4024740385)
- «Please add a comment explaining why we use a deprecated API here.» [rust#164744](https://github.com/rust-lang/rust/pull/164744#discussion_r4349780949)
- «Can you add a comment explaining where the +1 comes from.» [react#37730](https://github.com/react/react/pull/37730#discussion_r4195838826)

Naming:
- «this function name is a little misleading … maybe … invert the logic and call it `nameRequiresQu…`» [prometheus#19683](https://github.com/prometheus/prometheus/pull/19683#discussion_r4325586896)
- «nit: let's not mask imported `stats` package» [prometheus#19124](https://github.com/prometheus/prometheus/pull/19124#discussion_r3577703668)

API, types, conventions:
- «Please add `#[must_use]` to `FileTimes` … so that calling `times.set_modified(t);` … produces a compiler warning» [tokio#8504](https://github.com/tokio-rs/tokio/pull/8504#discussion_r4237519405)
- «Please use `AtomicPtr` instead of casting to/from usize» [tokio#8285](https://github.com/tokio-rs/tokio/pull/8285#discussion_r4079894185)
- «Let's define this as `libc::mode_t` so it always matches the platform» [tokio#8381](https://github.com/tokio-rs/tokio/pull/8381#discussion_r4281995870)
- «We always use strict equality unless there's a specific reason for weak equality.» [react#36971](https://github.com/react/react/pull/36971#discussion_r3590103280)
- «Is it worth adding a `default` case so we catch any future additions or erroneous data?» [prometheus#18051](https://github.com/prometheus/prometheus/pull/18051#discussion_r3712189887)

Correctness, errors, concurrency:
- «nit: apparently `BucketFraction` sorts `tc.buckets` in place, so it should be cloned first» [prometheus#19963](https://github.com/prometheus/prometheus/pull/19963#discussion_r4228607171)
- «nit: idexing of array not guarded for overflow» [prometheus#20026](https://github.com/prometheus/prometheus/pull/20026#discussion_r4282236144)
- «`err2` is nil at this point, so we need to return some non-nil error here» [kubernetes#142143](https://github.com/kubernetes/kubernetes/pull/142143#discussion_r4324564083)
- «If toJSON() returns this then this will result in stack overflow» [react#37147](https://github.com/react/react/pull/37147#discussion_r3815516982)
- «This is a pre-existing issue, but this should actually close the fd.» [tokio#8603](https://github.com/tokio-rs/tokio/pull/8603#discussion_r4237605684)

Performance (with a scale estimate):
- «A large `npm install` … can send tens of thousands of `Created` events, so we'd make tens of thousands of disk calls» [TypeScript#64909](https://github.com/microsoft/TypeScript/pull/64909#discussion_r4342906053)
- «This'd be faster as a `strings.Builder` …, but, it's probably not called enough to matter» [TypeScript#64372](https://github.com/microsoft/TypeScript/pull/64372#discussion_r4076370572)

## Nit eval scenarios (15)

Change → senior → value → noise (must yield 0 candidates or REJECTED).

**Rust**
1. A new builder `fn set_x(mut self, …) -> Self` without `#[must_use]`. → «add `#[must_use]`: `b.set_x(t);` silently drops the result». Noise: «add doc comments to the struct».
2. `AtomicUsize` stores a pointer via `as usize`. → «use `AtomicPtr<T>`». Noise: «unsafe is dangerous, double-check it».
3. A new helper + unit test for `i32::try_from(x).unwrap_or(i32::MAX)` with a single call site. → «inline it, the helper and test are not needed». Noise: «add a test for negative values of the helper».
4. A comment on a fix: «there used to be bug X here, so…». → «describe the invariant, the reader doesn't know about the bug». Noise: «add more comments».
5. A loop re-reads a value already computed before the loop. → «why re-read it, there's `v` above?» Noise: «extract the loop into a function».

**TS/React**
6. `if (a == null || b == c)` in code that uses `===` everywhere. → «we use strict equality unless there's a reason» (for `== null` it's fine, leave it). Noise: «enable strict mode in tsconfig».
7. A test replaces `console.error = jest.fn()` and does not restore it. → «`jest.spyOn`, otherwise it leaks into other tests; we don't silence errors in tests». Noise: «add more tests».
8. `useEffect` sets state derived from props. → «compute it during render/in the handler: an effect after paint produces a frame with stale UI». Noise: «wrap it in `useMemo` for performance» without measurement.
9. `index + 1` without explanation in an offset calculation. → «where does +1 come from? comment». Noise: «extract 1 into a constant `ONE`».
10. In a parser PR — a formatting change in an unrelated module. → «Looks unrelated — separate PR?» Noise: remarks on the contents of the unrelated code.

**Go**
11. `stats := …` inside a function in a file that imports `stats`. → «don't shadow the `stats` import» + a suggestion with another name. Noise: «rename to something more descriptive».
12. `var out []T; for … { out = append(out, …) }` with a known `len(in)`. → «`make([]T, 0, len(in))`». Noise: preallocate/`sync.Pool` on a cold path with unknown size.
13. A function sorts its argument in place; the test passes a slice from the case table. → «nit: the function sorts in place, clone it». Noise: «use `sort.Slice` instead of `slices.Sort`».
14. Behavior changed, but the comment above says «this is constant». → «the comment is stale, fix it». Noise: «add godoc to all functions».
15. `isPrintable(name)` actually checks «whether quotes are needed». → «the name is misleading; `nameRequiresQuoting` with inversion». Noise: «`isPrintable` → `checkPrintable`».

Negative control: `else` after `return` and formatting when `gofmt`/`revive`/clippy run in CI — duplicates the linter.

## Recommendations for discovery

1. A nit/improvement must have: an anchor, a value mechanism (which future error/misunderstanding it prevents) and
   a concrete fix; without all three it is not a candidate. Prefer `suggestion` text.
2. The strongest nit is a divergence from something that already exists in the repository: a helper, pattern, neighboring test, convention. Require
   an anchor to the example (`search_source`); «that's the usual practice in general» is `PREFERENCE`.
3. Do not suggest what rustfmt, clippy, gofmt, go vet, eslint, prettier, tsc catch; formatting, import order,
   whitespace are forbidden topics unless there is an accepted rule.
4. High value at low noise: a stale/false comment, a why-comment for a magic number or a deprecated
   API, name shadowing, argument mutation, `#[must_use]`/`default`, a test that cannot fail or passes
   without the fix, a test of internals instead of the public API, an unrelated hunk in a PR.
5. Uncertainty is a question with a concrete scenario («what happens with an empty X?»), not a statement; such a candidate
   stays `UNRESOLVED`, not a nit. Questions «for oneself» and praise are not emitted (Bosu: not useful).
6. A performance nit only with a scale argument (hot path, input size); otherwise do not emit it.
7. Prefer removal and inlining over adding an abstraction; suggest a new helper only with ≥3 repetitions in the diff.
8. Problems in untouched code get at most one «pre-existing» note, not a nit on the change.
9. Limit: no more than ~3 nits per review unit, and fewer on large PRs (usefulness drops with the number of files);
   select by item 4. Zero nits is a normal answer (already in the prompt).
10. Output wording: «nit:» + location + reason + fix, 1–2 sentences, identifiers in backticks.
