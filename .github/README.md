# .github

GitHub-native configuration for this repository: CI/CD workflows and dependency
automation. This is repo plumbing, not part of the routing crate.

- `workflows/` — GitHub Actions pipelines (test, container build, test-env deploy, CLI-flag audit).
- `dependabot.yml` — weekly automated dependency-update PRs for Cargo crates and GitHub Actions.
