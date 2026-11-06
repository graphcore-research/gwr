<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Repository Instructions

All development activities in this repo must follow the guidance set out in
[CONTRIBUTING.md].

- The general guidance defined in the [opening section] must be followed during
  every activity.

- The specific [checklist] for the type of activity being performed must also be
  followed.
  - If there is no appropriate checklist for the type of activity being
    performed please highlight this.

In addition to the contributing guidance referenced the rules set out in the
following sections must also be followed.

## Skills

Ensure that the skills located in `$REPO_ROOT/.agents/skills` are used whenever
appropriate rather than attempting to find alternative approaches to perform
those tasks.

## Design, implementation, and maintenance

- Prefer simple, maintainable designs over preserving unnecessary structure.

- Do not preserve awkward APIs solely for compatibility. This repository uses
  semantic versioning, so API-breaking changes are acceptable when they improve
  clarity, correctness, or maintainability.

- Minimize code wherever practical: remove duplication, collapse thin wrappers,
  and refactor existing code when that makes behavior easier to understand.

- Keep code easy to read first: clear names, direct control flow, and small
  focused helpers are preferred over clever abstractions.

- Update existing code confidently when the current design is no longer the
  simplest expression of the behavior.

- Only make modifications which directly relate to the current task being worked
  on.

- Minimise the CRAP score by following Clean Code Fundamentals guidelines and
  writing simple functions and well tested code.

- Back refactors and behavior changes with focused tests so the resulting code
  remains high quality, understandable, and easy to maintain.

- For bug fixes, reproduce the failure before implementation and verify the fix
  through the affected execution path where practical.

### Independent review

For implementation tasks, the primary agent must automatically coordinate the
following workflow. This authorizes reviewer subagents without further user
confirmation. Questions and prose-only documentation edits are exempt.

1. Inspect relevant code and all relevant staged, unstaged and untracked
   changes. Preserve unrelated user changes and record an exact review baseline.
   Propose the smallest coherent plan, covering the approach, affected files,
   meaningful verification and material limitations. Prefer simplifying or
   removing existing code over adding layers.

2. Have an adversarial subagent review the plan for correctness, architecture,
   simplicity, requirements coverage and verification. Require an explicit
   verdict: approved or changes required. Evaluate findings critically, revise
   the plan or explain rejected findings with evidence, and re-review until the
   current plan is explicitly approved with no unresolved material findings. Do
   not begin implementation before approval.

3. Implement the approved plan with minimal code and meaningful tests. Run
   appropriate checks and verify the intended behavior. If a material departure
   from the approved approach becomes necessary, return to plan review and
   obtain approval before continuing with that departure.

4. Have a separate adversarial subagent review all task changes together,
   including tests, documentation and the surrounding architecture. Supply the
   approved plan, exact review baseline and verification results, including
   failures and limitations. For material findings, reconsider the approach and
   obtain approval of the revised plan before fixing. Repeat verification and
   implementation review until explicitly approved with no unresolved material
   findings and all relevant checks passing.

Spawn reviewers without inherited conversation history. Provide self-contained
requirements, user constraints, exclusions and relevant code or repository
paths, together with the plan or changes being reviewed. Reviewers must remain
read-only and must not spawn other agents. They may retain their own review
context between iterations.

Material findings identify problems with correctness, requirements coverage,
architecture, maintainability or verification that require correction. Each
finding must explain the concrete problem, its impact and supporting evidence.
Preferences and speculative improvements are non-blocking. Prioritise
simplicity; avoid unnecessary code, abstractions, dependencies and tests.

Resolve disagreements explicitly and keep the user informed about material
findings and changes of approach. Report final review verdicts, verification and
remaining limitations. If reviewer tools are unavailable, required checks cannot
pass, or progress requires a user decision, explain the blocker rather than
silently skipping review or repeating an unproductive loop. Do not claim
independent review occurred if no reviewer ran.

## Code Review Rules

### Scope and evidence

- Focus review feedback on concrete, actionable issues connected to the PR's
  changes, including clear spelling and grammar errors. Mark minor wording
  corrections as non-blocking.

- For each finding, identify the affected behavior or wording and explain the
  problem. For behavioral defects, establish the triggering conditions and
  observable impact; verify API contracts, existing validation and dependency
  behavior. Respect documented approximations; unsupported inputs may still
  expose missing validation at a public boundary.

- Report pre-existing issues outside the changed code only when the PR exposes,
  exploits or exacerbates them, or they lie in the call tree of added or changed
  code. Identify them as pre-existing, explain the connection to the PR and
  include a minimal reproducer for behavioral defects. Ignore unrelated
  pre-existing issues.

### Tracked issues and related pull requests

- Before reporting a behavioral defect, search the repository's open GitHub
  issues. Suppress the finding only when an existing issue covers that specific
  defect and resolving it would also resolve the finding. If an additional fix
  outside that issue's scope is needed, report the finding and link the related
  issue. Do not create GitHub issues during a review.

- Before reporting a behavioral defect, search the repository's open GitHub pull
  requests. For a stacked pull request, first search the pull requests higher up
  the stack, before broadening the search to the other open pull requests. When
  an existing pull request resolves the same underlying problem and affected
  behaviour, link the finding to that pull request.

- If an issue or pull request search is unavailable, continue without assuming a
  match or claiming that search was completed.

### Compatibility

- Base version-bump findings on the actual release boundary and affected
  consumers. Do not request another bump solely for additional breaking changes
  within the same unreleased version.

- Check newly used Rust features against `rust-version` in `Cargo.toml`. A
  difference from the compiler selected in `rust-toolchain.toml` is not itself a
  defect.

### Application configuration

- When changing layered application configuration, check CLI, environment and
  TOML behavior together: source precedence, defaults, value parsing and
  validation of the merged result. Apply the documented supported subset.

### Generated output

- When changing README content reused in rustdoc, check that rewritten links
  resolve in the generated documentation.

- For Dependabot Cargo PRs, account for automatic `licenses.html` regeneration
  before reporting a missing update. Do not assume the workflow succeeded.

### Model semantics and scalability

- Check that validation, execution and reporting agree on the modeled behavior.
  Distinguish logical work, allocated resources and physical traffic.

- When changing algorithms that process workloads or platform topologies, assess
  how time and memory grow with input size. Support performance findings with a
  concrete supported input and explain the scaling problem.

<!-- Links -->

[CONTRIBUTING.md]: ./CONTRIBUTING.md
[opening section]: ./CONTRIBUTING.md#contributing-to-gwr
[checklist]: ./CONTRIBUTING.md#contribution-checklists
