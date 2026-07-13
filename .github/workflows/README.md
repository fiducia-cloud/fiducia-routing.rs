# .github/workflows

GitHub Actions pipelines for the fiducia-routing crate. Each file is one workflow:

- `ci.yml` — mandatory formatting, locked all-target Clippy/tests, and a pinned
  cargo-audit on push/PR.
  Checks out the reviewed full `fiducia-interfaces` commit next to the repo so
  the path dependency resolves without following a moving branch.
- `docker.yml` — builds and pushes the `fiducia-region` image to ghcr.io
  (`latest` + commit-SHA tags) on pushes to `main`.
- `deploy-test.yml` — rolls the `fiducia-test` Kubernetes namespace to the
  freshly built image. No-op unless the `KUBE_CONFIG_TEST` secret is present.
- `cli-flags.yml` — audits `.cli-flags.toml` with the pinned `flags2env`
  submodule whenever the CLI flag schema, scripts, or submodule change.
