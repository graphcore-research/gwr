<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Setting up `gwr-bot` for the Dependabot license-regeneration workflow

<div align="center">

![G W R - B O T full logo](gwr-9400-bot-driver-sketch.png)

</div>

The [`regenerate-licenses-on-dependabot`](../workflows/regenerate-licenses-on-dependabot.yaml)
workflow amends Dependabot's commits and force-pushes the result so that
CI runs against a branch with an up-to-date `licenses.html`. It needs a
token with `contents: write` on the repository, because:

- The default `GITHUB_TOKEN` supplied to Dependabot-triggered workflows is
  read-only and cannot push.
- Pushes made with `GITHUB_TOKEN` do not trigger downstream workflow runs,
  so even if it were writable, CI would not re-run on the amended commit.

The recommended way to provide such a token is a dedicated GitHub App —
called `gwr-bot` in this repo — whose installation token is minted per
workflow run. An App is preferable to a Personal Access Token because it
is not tied to a human account, has no expiry, is auditable in the org's
audit log, and can be scoped to only the repositories that need it.

## 1. Create the GitHub App

GitHub Apps are always owned by either a user or an organisation. If you
do not have admin rights on `graphcore-research`, create the App under
your own user account — it can still be installed on the `gwr` repository
provided a repository admin approves the installation (see step 3).

1. Go to
   <https://github.com/settings/apps/new>
   (**Your profile → Settings → Developer settings → GitHub Apps → New
   GitHub App**).
   - If you *do* have `graphcore-research` admin rights and would rather
     the App live under the org, use
     <https://github.com/organizations/graphcore-research/settings/apps/new>
     instead. Everything else in this guide is identical.
2. Fill in the form as follows:
   - **GitHub App name**: `gwr-bot` (must be unique across GitHub; append
     a suffix such as `gwr-bot-<yourhandle>` if the name is taken).
   - **Homepage URL**: `https://github.com/graphcore-research/gwr`
   - **Webhook**: uncheck **Active**. This App does not need to receive
     events; it is only used to mint installation tokens.
   - **Repository permissions**:
     - **Contents**: `Read and write` — needed to push the amended commit.
     - **Metadata**: `Read-only` — this is added automatically and cannot
       be removed.
     - Leave everything else set to `No access`.
   - **Organization permissions**: none.
   - **Account permissions**: none.
   - **Where can this GitHub App be installed?**:
     - If you are creating the App under the `graphcore-research`
       organisation, choose **Only on this account**.
     - If you are creating the App under your own user account, choose
       **Any account**. This is required so that the App can be
       installed on a repository owned by a different account
       (`graphcore-research`); "Only on this account" would restrict
       installations to your own user account and you would not see
       `graphcore-research` in the install list in step 4.
3. Click **Create GitHub App**.

## 2. Upload a logo

1. On the App's page, scroll to **Display information** and click **Upload a
   logo...**.
2. Select [`.github/gwr-bot-logo.png`](gwr-bot-logo.png) image file.
3. Click **Upload**.
4. Click **Set new avatar**.
5. Set the **Badge background colour** to "#F9F3E9".


## 3. Generate and store a private key

1. On the App's settings page, scroll to **Private keys** and click
   **Generate a private key**. A `.pem` file is downloaded — treat it as
   a secret.
2. Note the **App ID** shown near the top of the settings page.

## 4. Install the App on the `gwr` repository

1. On the App's settings page, click **Install App** in the left sidebar.
2. Next to `graphcore-research`, click **Install** (or **Request** if you
   are not an org owner — an org admin will need to approve the request).
3. Choose **Only select repositories** and pick just
   `graphcore-research/gwr`.
4. Click **Install**.

If a repository admin needs to approve the installation, direct them to
<https://github.com/graphcore-research/gwr/settings/installations> once
they have received the request notification.

## 5. Store the credentials as *Dependabot* secrets

The workflow runs in a Dependabot-triggered context, so the credentials
must live under **Dependabot** secrets, not regular Actions secrets.

Go to **Settings → Secrets and variables → Dependabot → New repository
secret** and add:

| Name                  | Value                                                 |
| --------------------- | ----------------------------------------------------- |
| `GWR_BOT_APP_ID`      | The App ID from step 3.                               |
| `GWR_BOT_PRIVATE_KEY` | The full contents of the `.pem` file from step 3, including the `-----BEGIN…-----` / `-----END…-----` lines. |

## 6. Confirm the workflow is wired up

The workflow at
[`.github/workflows/regenerate-licenses-on-dependabot.yaml`](../workflows/regenerate-licenses-on-dependabot.yaml)
is already set up to mint an installation token via
[`actions/create-github-app-token`][create-app-token] from the two
secrets you just created. No workflow edits are required.

## 7. Verify

1. Trigger a Dependabot run manually (**Insights → Dependency graph →
   Dependabot → Recent update jobs → Check for updates**) or wait for the
   next scheduled run.
2. When a `dependabot/cargo/*` PR appears, watch the
   `Regenerate dependency licenses on Dependabot cargo PRs` workflow run
   under **Actions**. It should succeed and produce a force-push.
3. On the PR, confirm that:
   - The tip commit is authored by `dependabot[bot]` and committed by
     `gwr-bot[bot]`.
   - CI has run against the amended commit (the `gate` job is not
     skipped, because `github.actor` is now `gwr-bot[bot]`, not
     `dependabot[bot]`).

## Rotating or revoking the App

- **Rotate the key**: generate a new private key on the App's settings
  page, update `GWR_BOT_PRIVATE_KEY`, then delete the old key. Multiple
  active keys are supported, so this can be done without downtime.
- **Revoke access to the repo**: go to
  <https://github.com/graphcore-research/gwr/settings/installations>,
  click **Configure** next to `gwr-bot`, then **Uninstall**. A repository
  admin can do this without needing access to the App itself.

[create-app-token]: https://github.com/actions/create-github-app-token
