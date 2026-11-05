<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Contributing to GWR

<!-- ANCHOR: intro -->

All contributions to GWR must follow the appropriate [contribution checklist].

Whilst GWR does not currently define its own LLM usage policy or guidelines it
does look to follow the Rust Forge's [LLM Usage Policy] and Rust Compiler
Development Guide chapter on [Working with LLMs].

We are currently unable to accept unsolicited pull requests.

<!-- Absolute links -->

[LLM Usage Policy]: https://forge.rust-lang.org/policies/llm-usage.html
[Working with LLMs]: https://rustc-dev-guide.rust-lang.org/llm-guidance.html

<!-- ANCHOR_END: intro -->

<!-- Relative links (must be defined wherever the `intro` anchor is used) -->

[contribution checklist]: #contribution-checklists

## Contribution checklists

<!-- ANCHOR: checklists -->

- [Issue found checklist](#issue-found-checklist)
- [Feature/fix development checklist](#featurefix-development-checklist)
- [Rust package documentation checklist](#rust-package-documentation-checklist)
- [Infrastructure documentation checklist](#infrastructure-documentation-checklist)
- [Code style checklist](#code-style-checklist)
- [Code review checklist](#code-review-checklist)

### Issue found checklist

1. Check for an existing [issue] covering the feature or bug, create one if
   there isn't already one tracking the topic.

1. Add/update the [labels] on the issue, ensuring that at least all `A` and `C`
   labels that apply are set.

1. If setting an `M` label also set the Milestone field to match.

1. Ensure that the issue title and description, and the discussion messages are
   free from any proprietary information, intellectual property, or project
   names before creating them.

### Feature/fix development checklist

1. Specification

   Changes must have a specification against which reviewers can evaluate them.
   Ideally the specification will be drafted as a GitHub issue allowing us to
   keep conversations about the requirements and review of the implementation
   separate and on track.
   1. Follow the [issue found checklist](#issue-found-checklist).

   1. Assign the issue to yourself.

   1. Add detail as required to the issue such that a reviewer will have a clear
      understanding of the intent of the change.

1. Branching <a id="branching"></a>

   The [Branching the Repo] section provides background information, examples,
   and a diagram of the git workflow used within GWR.
   1. Select the appropriate base branch for the current type of work:
      - `main` for contributions being developed to state of production
        readiness.

      - `experimental` for rapid prototyping of ideas.

   1. Create a branch named with appropriate prefix:
      - `pr-` for short-lived branches used to open pull requests.

      - `feature-` for long running or collaborative development.

1. Testing
   1. Follow the [code style checklist](#code-style-checklist) when developing
      tests.

   1. Each test should have a single purpose.

   1. Ensure that test code is the simplest possible reproducer of the scenario
      to be tested.

   1. Tests should be DRY. Common setup and boilerplate code should be
      abstracted and shared.

   1. Ensure that any tests added for the sake of confirming old behaviours are
      no longer seen are removed once the implementation is complete.

   1. Run the code coverage and CRAP tooling to determine where further testing
      is required.

   1. Run both wall-clock and one-shot benchmarks to understand the potential
      performance impact of the change.

1. Implementation
   1. Follow the [code style checklist](#code-style-checklist).

1. Documentation <a id="documentation"></a>

   Documentation must remain both readable and understandable by the humans
   working on and with this repository. Documentation should aim to be concise,
   clear, and engaging to avoid fatiguing the reader. To ensure this all
   documentation must be created by humans.
   1. Follow the [Rust package](#rust-package-documentation-checklist) or
      [infrastructure](#infrastructure-documentation-checklist) documentation
      checklist as appropriate.

   1. Ensure that the [developer guide], i.e. the `gwr-developer-guide` package,
      is up to date and correctly reflects the changes being made.

1. Commits <a id="commits"></a>

   Commit messages are considered to be an integral part of the documentation of
   GWR, and as such the same guidance set out in [Documentation](#documentation)
   applies to the way that they are written. Package release and dependency
   update workflows are exempt from this requirement as they instead use a
   templated process to generate commit messages.
   1. Ensure the pre-commit hooks are installed. Run `prek install` to do this.

   1. A commit must only contain changes for a single coherent feature or fix.

   1. Ensure that each commit message correctly documents the change, detailing
      at least:
      - The rationale for making the change.

      - The rationale for implementing the change using the chosen approach when
        the choice was directed by a specific metric, e.g. performance, or when
        there are obviously multiple possible approaches.

   1. Commit messages must follow the [Conventional Commits] 1.0.0
      specification. This allows the automatic generation of both a top-level
      changelog covering the whole workspace as well as an individual changelog
      for each package within the workspace.

      Write a commit message in this form:
      1. The set of `type`s that can be used can be found in the
         `commit_parsers` array within the `release-plz.toml` configuration
         file. This array also details the sections each type will be included
         in within the changelog.

      1. The `optional scope` should be used to detail the name of the updated
         package. For example:

         ```text
         fix(gwr-developer-guide): check for panic when running mdBook tests
         ```

         1. If the change applied to multiple, but not all packages, the names
            should be comma seperated. For example:

         ```text
         feat(gwr-track,gwr-spotter): capnp binary file support
         ```

         1. If the change applies to all Rust packages, infrastructure, CI, or
            general configuration the scope can be omitted. For example:

         ```text
         docs: add example use for all public APIs
         ```

         ```text
         infra: set default pre-commit hooks to install
         ```

      1. Keep the subject line as short as possible whilst remaining clear.

      1. Wrap body lines at 72 characters.

      1. Ensure that both the content of the commit and the commit message are
         free from any proprietary information, intellectual property, or
         project names before creating them.

1. Pull Request
   1. A pull request must only contain commits that together make up a single
      coherent set of features or fixes, i.e. a set of commits which should be
      reviewed together.

   1. Pull requests should be kept small to ensure they remain reviewable.
      Prefer creating a stack of small PRs rather than a single large one when
      related features or fixes can sensibly be reviewed separately.

   1. Add the appropriate `A` [labels] to the pull request, and optionally an
      `M` label.

   1. If setting an `M` label also set the Milestone field to match.

   1. Commits should be amended when addressing review feedback on a PR as they
      will not be squashed when the PR is merged.

   1. Ensure that both the pull request title and description, and the
      discussion messages are free from any proprietary information,
      intellectual property, or project names before creating them.

### Rust package documentation checklist

1. Ensure that the README.md at the package root remains correct and up to date
   with the behaviour of the package.

1. Public APIs must be fully documented using `rustdoc` such that they are
   described by the [API documentation].

### Infrastructure documentation checklist

1. Ensure that the README.md at the repo root remains correct and up to date
   with the behaviour of the infrastructure.

1. Ensure that the CONTRIBUTING.md at the repo root remains correct and up to
   date with the behaviour of the infrastructure.

1. Ensure that the AGENTS.md at the repo root remains correct and up to date
   with the behaviour of the infrastructure.

1. Ensure that the skills defined in `$REPO_ROOT/.agents/skills` remain correct
   and up to date with the behaviour of the infrastructure.

### Code style checklist

1. Do not use magic numbers, define constants with meaningful names.

1. Variable names should be chosen for clarity, not brevity.

1. Where possible avoid the use of fully qualified types. Prefer `use`
   statements instead.

1. Where a macro or type has been defined for a particular construct use it
   consistently.

   For example, rather than returning `Err(SimError(format!("...")))` prefer
   returning `sim_error!("...")`.

1. Arrange functions for ease of readability, from top to bottom of the file,
   i.e. dependants come before dependencies.

   This does not apply to `#[test]` functions which should be placed at the
   bottom of the file, and after any test helper functions when inside a
   `#[cfg(test)] mod`.

1. Ensure that 1 blank line is inserted between:
   - Top-level structs.

   - `impl` blocks.

   - Functions.

   Note this is not something that rustfmt can currently be configured to
   enforce, so must be done manually.

### Code review checklist

1. Ensure the
   [Feature/fix development checklist](#featurefix-development-checklist) has
   been followed.

1. Check for performance changes using the benchmark results captured by CI.
   Significant performance regressions should be documented as part of the
   commit.

<!-- Absolute links -->

[API documentation]:
  https://graphcore-research.github.io/gwr/rustdoc/gwr_engine/index.html
[Conventional Commits]: https://www.conventionalcommits.org/en/v1.0.0
[developer guide]: https://graphcore-research.github.io/gwr/html/index.html
[issue]: https://github.com/graphcore-research/gwr/issues
[labels]: https://github.com/graphcore-research/gwr/labels

<!-- ANCHOR_END: checklists -->

<!-- Relative links (must be defined wherever the `checklists` anchor is used) -->

[Branching the Repo]: ./README.md#branching-the-repo
