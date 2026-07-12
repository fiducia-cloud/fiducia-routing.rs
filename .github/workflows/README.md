# .github/workflows

GitHub Actions pipelines for the fiducia-routing crate. Each file is one workflow:

- `ci.yml` — fmt, clippy, `cargo test`, and cargo-audit on push/PR. Checks out
  `fiducia-interfaces` next to the repo so the path dependency resolves.
- `docker.yml` — builds and pushes the `fiducia-region` image to ghcr.io
  (`latest` + commit-SHA tags) on pushes to `main`.
- `deploy-test.yml` — rolls the `fiducia-test` Kubernetes namespace to the
  freshly built image. No-op unless the `KUBE_CONFIG_TEST` secret is present.
- `cli-flags.yml` — audits `.cli-flags.toml` with the pinned `flags2env`
  submodule whenever the CLI flag schema, scripts, or submodule change.
