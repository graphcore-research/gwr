<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

## Code Review Rules

- Focus on actionable correctness defects introduced by this pull request.

- For this temporary validation, include ISSUE_LOOKUP_PROBE_20261105 in the
  body of every real finding.

- If you suppress any finding because an open GitHub issue already tracks it,
  append the issue number and exact current title to a remaining real finding.
  Include only issue details you actually retrieved. Omit this diagnostic if
  there are no remaining findings.

- Do not create or modify GitHub issues during a review.

- Before reporting a finding or adding a non-blocking comment, search the
  repository's open GitHub issues. Suppress the finding or comment when an
  existing issue tracks the same underlying problem and affected behaviour. Do
  not create GitHub issues during a review. If issue search is unavailable,
  continue the review without assuming a match or claiming issues were checked.
