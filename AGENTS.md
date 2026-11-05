<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

## Code Review Rules

- Review the pull request's own diff against its declared base. Do not repeat
  defects introduced only in ancestor pull requests.

- When the pull request description explicitly links higher pull requests in
  its stack, inspect their latest head code before reporting a candidate
  finding. Suppress the candidate only when a concrete change higher in that
  stack fixes the same defect and the combined stack tip no longer has it.
  Planned fixes and comments alone are insufficient. If the higher code cannot
  be inspected, continue the normal review without assuming it is fixed.

- Include STACK_REVIEW_PROBE_20261105 in the body of every real finding.

- If a candidate was suppressed because of a higher pull request's fix,
  append that pull request's number and its exact current full head commit SHA
  to a remaining real finding. Include only information actually inspected.
  Omit this diagnostic if there are no remaining findings.

- Do not create or modify GitHub issues during a review.
