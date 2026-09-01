# Main branch protection

The `main` branch is protected and should accept changes only through reviewed pull requests.

Maintainers should configure the repository with the following settings:

- Require a pull request before merging, with at least one approving review.
- Require CODEOWNERS review for changes under `.github/`, `contracts/`, and release-sensitive paths.
- Dismiss stale approvals when new commits are pushed.
- Require branches to be up to date before merging.
- Require conversation resolution and prevent force pushes or branch deletion.
- Do not permit direct pushes to `main`, including administrator bypasses except for emergency recovery.
- Require the following status checks to pass before merging (see [CI workflow](workflows/ci.yml)):
  - `build-and-test`
  - `go-sdk`
  - `python-examples`
  - `npm-packages (dir: packages/sdk)`
  - `npm-packages (dir: packages/ui-components)`
  - `npm-packages (dir: packages/ts-advanced-client)`
  - `npm-packages (dir: services/graphql-api)`
  - `npm-packages (dir: services/webhook-streamer)`
  - `npm-packages (dir: services/health-dashboard)`
  - `npm-packages (dir: examples/client)`

  > **Keeping this list in sync with `ci.yml`**
  > Every job name and matrix entry above must match the names in `.github/workflows/ci.yml` verbatim.
  > If jobs are added, removed, or renamed, update this list in the same PR.

## Testnet Smoke Test

The [Testnet Smoke Test](workflows/smoke-test.yml) workflow is **not** a required PR check.
It only runs on `workflow_dispatch` (manual trigger) and requires the `TESTNET_SECRET_KEY` secret to
fund and sign real testnet transactions, so it cannot run automatically on pull requests or from forks.

Maintainers should run it **manually before a release** or as part of the release process to verify
deployed contracts behave correctly on testnet. It is a post-merge / pre-release quality gate, not a
PR-blocking check.

## Settings drift

These settings encode the policy described in `CONTRIBUTING.md`; repository administrators should
periodically verify that the configured rules have not drifted. Any time the CI workflow's job names
or matrix entries change, update the required-check list above and confirm the branch-protection
settings still reference valid check names.
