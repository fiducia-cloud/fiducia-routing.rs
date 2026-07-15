# .github/workflows

GitHub Actions pipelines for the fiducia-routing crate. Each file is one workflow:

- `ci.yml` — mandatory formatting, locked all-target Clippy/tests, and a pinned
  cargo-audit on push/PR.
  Checks out the reviewed full `fiducia-interfaces` commit next to the repo so
  the path dependency resolves without following a moving branch.
- `docker.yml` — builds and pushes the `fiducia-region` image to ghcr.io with
  only its immutable commit-SHA tag, maximum provenance, and an SBOM.
- `cli-flags.yml` — audits `.cli-flags.toml` with the pinned `flags2env`
  submodule whenever the CLI flag schema, scripts, or submodule change.

This repository contains no environment credentials or rollout workflow;
deployment is owned by `fiducia-monorepo`.

## Security baseline

Every executable workflow uses explicit least-privilege permissions, immutable
third-party action or container references, non-persisted checkout credentials,
concurrency control, and a job timeout. The main CI workflow validates this
directory with the digest-pinned actionlint container. Environment mutation is
forbidden unless this README documents a repository-specific platform exception.
