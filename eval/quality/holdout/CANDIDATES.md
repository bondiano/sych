# Holdout candidates

Unlabelled changes for the owner to choose from (spec §27.2). They were picked from commit metadata only: subject,
file count, size and file types. Their diffs were not read, and none of them is in `../dev`. Sych did not look at
them while tuning prompts or rules. Labelling, the choice and the split are the owner's.

How a fix commit becomes a case: the fix itself is a `safe` case (or an `unknown` one). The commit that introduced
the bug, found with `git log -L` or `git blame` on the fixed lines, is the `unsafe` case. Both use
`repository_fixture` with `base_ref: <commit>~1` and `head_ref: <commit>`.
When what the fix removes is only a nit (a style or rules-of-the-framework issue with no shown consequence),
the introducing commit is a `safe` case that expects a `nitpick` finding instead, and the two are not a pair: an
`unsafe` case needs a defect.

## Labelled so far

Drafted by an agent that did not read `../dev`, prompts, rules or the analysis code; the owner reviews them.

| case pair / id | variant | commit | source |
|---|---|---|---|
| vkusori-swipe-dismiss-ref-unsafe / -safe | unsafe with a nitpick / safe | 19ca179d0 / 280d692b1 | introducer listed below; the fix is not; not a pair: 280d692b1 fixes only the nit. The defect label was added after the first run reported it |
| vkusori-with-resolvers-safe | safe | 8cfa192de | listed; no pair, see below |
| vkusori-storage-open-retry-safe | safe | 7cd85f890 | listed; no pair, see below |
| vkusori-food-cluster-timeout-unknown | unknown | 707ba48a2 | listed |
| vkusori-screen-lead-under-sheet | unsafe / safe | 14802709c / e31c97148 | not listed: fix found by blame over small `fix(...)` commits; introducer ~205 lines |
| vkusori-guest-profile-identities | unsafe / safe | a349b4fba / 03e09e1db | not listed: introducer is 585 lines, ~350 without stories, tests and CSS, above the usual size; kept because the bug is in two short hunks of it |

The commits outside these tables were not checked against `../dev` (that would mean reading it): the owner checks that
bc0938d03, 0222aa17a, fcadf8d6c, 00d52812a, 280d692b1, 14802709c, e31c97148, a349b4fba and 03e09e1db are not
development cases.

Moved to `../dev` after the first run: `vkusori-signin-consent-resume` (bc0938d03 / 0222aa17a) and
`vkusori-faro-error-duplicates` (fcadf8d6c / 00d52812a). Their misses were diagnosed and the pipeline was
changed from them, so they no longer measure independently.

Dropped on review: 55c82f785 as an `unknown` case, because the repository answers what the diff does not
(the sync hook runs in the push transaction under the user's lock, taken first), so it is not undecidable.

Dropped as pairs, because the commit that introduced the bug is large or the bug does not show in its diff:
7cd85f890 and 8cfa192de (both introduced in 03d8f9b64, ~1270 lines; the service worker half in 7c026d7a3, ~1130),
01d77a1f7 (d18e27efe, ~3700), 81782b26f (FSD move 7aced1d29, 1179 files), c73c7d0c7 (4efabdac3, ~36 000),
a4cccb7ef (371eaf4dd, ~850), 452de6460 (an omitted skeleton variant, a layout jump that no diff shows).
c2ac3f3c6 is a planned follow-up of a deliberate shortcut, not a fix, and was left out.

Dropped in the second pass (every `fix(...)` commit with ≤ 200 changed TS/TSX lines, introducers found by blaming
the lines each fix changes):

- introducer large: 226853238 and 1ce8754b9 (341688f47, ~16 000 lines; e7110922f), 2680f9a46 (590ec49c5, ~10 000),
  3f9dade43 (c18d25bf3, ~2000), 5efc0a0e5 (a1f2d84be, ~4700), bdc6a358d (3015aa34d and e7110922f), 31af8bb6a,
  db619b647 and 7b20837a2 (facafba00, ~600), 59754ab01 (a 26-file fix; 079f5443f), e2d4a884f and b9169c9c8
  (design changes over 3e0e768a1, 2a32780b9), 742ef690e, 293c5e234, 945cf04ce, 319afc1f1, eda90601e.
- bug not visible in the introducer's diff: 7cc0213b5 (4bbeea884; an iOS Safari offset kept from the keyboard),
  ebb4f7198 (fea8b025d; the registry hands the initial state to subscribers on its first write, a library
  behaviour), 5b758a39e (619a9a3b9, 65cc4b764; the service that reads onlineAtom on its turn came later), 38cb7e5bc
  (eb0e87760; a resize step count tuned to Safari's missing WebP encoder), f1cec4d14 (expo-network's repeated
  events), bcef60c6e → 999b7ecc5 (the fix is ~470 lines over 20 files and why the report blocked recovery is not in
  bcef60c6e), 0a1044c2e (183 lines, several purposes).
- not a code defect: fea8b025d (repeated events counted as turns for the E2E harness), 96f164b86 (aria-busy for an
  E2E wait), CSS-only layout fixes.

## Vkusori: TS/React (supported now)

| commit | files | lines | subject |
|---|---|---|---|
| 7cd85f890 | 2 | 38 | fix(pwa): a storage worker that cannot start reaches recovery in 2 s, not 20 s |
| 8cfa192de | 2 | 50 | fix(pwa): open the device database without Promise.withResolvers, which iOS 17.0–17.3 lack |
| 81782b26f | 2 | 52 | fix(pwa): the app-ready envelope names the day screen by its lazy index.ts |
| 01d77a1f7 | 2 | 25 | fix(pwa): "Choose days manually" stays while the pair of days reloads |
| 452de6460 | 3 | 38 | fix(pwa): the lazy day route waits in the day's own columns |
| c73c7d0c7 | 7 | 46 | fix(pwa): a cited observation keeps the focus after "Open observation" |
| c2ac3f3c6 | 3 | 28 | feat(core): a meal item of a BLS, CIQUAL or MEXT product keeps its source |
| 03ef13d2d | 3 | 142 | perf(pwa): the coach route gates load with their routes, not with the router |
| 25337f305 | 7 | 123 | fix(pwa): a page arrives whole on navigation, no fade-in blink |
| a4cccb7ef | 7 | 163 | fix(pwa): the product card of BLS, CIQUAL and MEXT names its own source |
| 19ca179d0 | 6 | 223 | feat(pwa): swipe a toast away, down or sideways, as an Android snackbar |
| 6d0560b4d | 11 | 361 | feat(pwa): "Another date" opens a month whose weeks start on the profile's day |
| 1fa090fc5 | 11 | 341 | feat(core): signInWithApple over the shared finishSignIn |

## Vkusori: Elixir (no Sych backend; good `unknown` cases)

| commit | files | lines | subject |
|---|---|---|---|
| c83560c13 | 6 | 48 | feat(ops): mix precommit rejects unsafe migrations after the last deployed one |
| 707ba48a2 | 2 | 88 | fix(foods): a hanging food_cluster batch is killed after 5 minutes |
| 55c82f785 | 5 | 125 | fix(reminders): a planner run that read the settings before a replan writes nothing |
| b03f86b2e | 5 | 154 | fix(foods): clusters by word similarity, only equal names without the model |
| 9a3952ba1 | 8 | 224 | fix(foods): the food search finds Cyrillic capitals and typos in the C locale |

## Sych: Rust

These candidates refer to Sych's pre-squash history and require an archival clone. That history was short and
mostly large commits, some written while Sych was being tuned. Only these were of a reviewable size and predated
the quality work:

| commit | files | lines | subject |
|---|---|---|---|
| af059d5 | 12 | 525 | feat(semantic): path classes, pull diagnostics and cargo configuration |
| c634054 | 14 | 295 | feat(analysis): batched discovery, inline output schemas, per-request output cap |

The rest are above 800 lines or come from the quality work itself. For Rust it may be better to hand-write
scenarios in the format of `case.yaml.template` without reading `../dev`.
