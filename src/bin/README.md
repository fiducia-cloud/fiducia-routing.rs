# src/bin

Binary targets for the crate.

- `fiducia-region.rs` — the `fiducia-region` CLI. Resolves a customer region
  (explicit `--region`, or nearest to `--lat`/`--lon`) and prints the shard a
  key maps to, so operators can inspect and debug region-aware routing from the
  shell. Also lists configured regions and their backing clusters (`--list`).
  Reads flags directly or the `FIDUCIA_*` env vars set by
  `scripts/with-flags2env.sh`; shipped as the container image built by
  `docker.yml`.
