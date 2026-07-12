# src

Source for the `fiducia-routing` crate — the single source of truth for how a
key maps to a shard across the fiducia.cloud platform.

- `lib.rs` — the library. The frozen FNV-1a hash (`fnv1a`), `shard_for`
  (`hash % shard_count`), region-aware sharding (`Region`, `shard_for_region`,
  `route_shard`), the reserved lock/service-discovery coordination keys, and the
  golden-vector tests that pin the hash so the mapping can never silently drift.
- `bin/` — the `fiducia-region` operator CLI built on top of the library.

Consumers (`fiducia-node`, `fiducia-load-balance`, `fiducia-brain`) depend on
this crate rather than reimplementing the mapping, so every component agrees on
where a key lives.
