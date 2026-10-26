# Holdout candidates

Unlabelled changes for the owner to choose from (spec §27.2). They were picked from commit metadata only: subject,
file count, size and file types. Their diffs were not read, and none of them is in `../dev`. Sych did not look at
them while tuning prompts or rules. Labelling, the choice and the split are the owner's.

How a fix commit becomes a case: the fix itself is a `safe` case (or an `unknown` one). The commit that introduced
the bug, found with `git log -L` or `git blame` on the fixed lines, is the `unsafe` case. Both use
`repository_fixture` with `base_ref: <commit>~1` and `head_ref: <commit>`.

## Vkusori: TS/React (supported now)

| commit | files | lines | subject |
|---|---|---|---|
| 7cd85f890 | 2 | 38 | fix(pwa): a storage worker that cannot start reaches recovery in 2 s, not 20 s |
| 8cfa192de | 2 | 50 | fix(pwa): open the device database without Promise.withResolvers, which iOS 17.0–17.3 lack |
| 81782b26f | 2 | 52 | fix(pwa): the app-ready envelope names the day screen by its lazy index.ts |
| 01d77a1f7 | 2 | 25 | fix(pwa): «Выбрать дни вручную» stays while the pair of days reloads |
| 452de6460 | 3 | 38 | fix(pwa): the lazy day route waits in the day's own columns |
| c73c7d0c7 | 7 | 46 | fix(pwa): a cited observation keeps the focus after «Открыть наблюдение» |
| c2ac3f3c6 | 3 | 28 | feat(core): a meal item of a BLS, CIQUAL or MEXT product keeps its source |
| 03ef13d2d | 3 | 142 | perf(pwa): the coach route gates load with their routes, not with the router |
| 25337f305 | 7 | 123 | fix(pwa): a page arrives whole on navigation, no fade-in blink |
| a4cccb7ef | 7 | 163 | fix(pwa): the product card of BLS, CIQUAL and MEXT names its own source |
| 19ca179d0 | 6 | 223 | feat(pwa): swipe a toast away, down or sideways, as an Android snackbar |
| 6d0560b4d | 11 | 361 | feat(pwa): «Другая дата» opens a month whose weeks start on the profile's day |
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
