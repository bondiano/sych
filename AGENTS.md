# Rules for agents

Sych reviews its own pull requests: every agent PR is a test of Sych itself. These rules are mandatory.

## Change flow

1. Work only on your own branch from a fresh `origin/main`. Direct pushes to `main` are blocked (protected on GitHub).
2. Before pushing: `just check`.
3. Open a PR: `gh pr create --base main --fill` (or with a meaningful description via `/pr`).
4. After **every** push, wait for the Sych review of the current head:

   ```sh
   scripts/wait-sych            # PR of the current branch; prints the summary and inline comments
   ```

   The review arrives within a few minutes; the script waits up to 30 minutes (`SYCH_WAIT_SECONDS`). Without
   a review of the current head the work is not done and the PR is not merged.
5. Go through every finding and UNKNOWN in the summary:
   - a real problem — fix it, push, run `scripts/wait-sych` again;
   - a false positive or noise — do not bend the code to the reviewer; reply in the PR with a short comment
     explaining why, and describe the case in your report to the user: it is a Sych defect, which is exactly
     what self-review is for;
   - `coverage: partial` or gaps — list them in your report, never present them as "no problems".
6. If no review arrives (exit 1), do not work around it: tell the user, with the PR and the head SHA.

## Never

- Merge a PR without a Sych review of the current head and green CI.
- Change the reviewer, its models, budgets or rules in a PR to get a "clean" review: the production reviewer
  is built from `main` and pinned by digest outside this repository, so it would not affect the review of
  that PR anyway.
- Comment `/sych rerun|deep` without a reason: every run costs money.

How the bot works and its runbook: [docs/SERVER.md](docs/SERVER.md#self-review-bondianosych).
