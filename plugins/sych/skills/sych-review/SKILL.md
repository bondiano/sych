---
name: sych-review
description: Review a branch with Sych before pushing it or opening a pull request, and act on its findings, UNKNOWN obligations and coverage gaps. Use when the user asks for a Sych review, or before a push in a repository that reviews with Sych.
---

# Review with Sych before a push

1. Run the review as the `/sych:review` command describes: committed changes only, against the remote default
   branch; a real model profile only with the user's consent.
2. Go through every finding and every UNKNOWN obligation:
   - a real problem: fix it, commit and review again;
   - a false positive: do not bend the code to the reviewer; explain why in the pull request;
   - `coverage: PARTIAL` or listed gaps: report them, never as "no problems".
3. Put the recommendation, the coverage and the open gaps into the pull request description.

Sych never approves or blocks: its recommendation is advice, and a merge stays the user's decision.
