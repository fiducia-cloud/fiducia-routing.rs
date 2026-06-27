# fiducia-routing

The shared **key → shard routing** library for [fiducia.cloud](https://fiducia.cloud)
— the single source of truth for how a key maps to a shard. Every component
depends on this instead of carrying its own copy, so the mapping **cannot
drift**.

## Why this exists

If two components computed `key → shard` even slightly differently, the cluster
would split its brain: the load balancer would route a request to one shard
while the data plane stored it in another, and locks/keys would silently land in
the wrong Raft group. Centralizing the hash makes that class of bug impossible.

## API

```rust
pub type ShardId = u32;
pub fn fnv1a(s: &str) -> u32;                       // the (frozen) hash
pub fn shard_for(key: &str, shard_count: u32) -> ShardId;  // hash(key) % shard_count

// Region-aware (geo-local) sharding — opt-in:
pub fn region_index(region: &str, regions: &[&str]) -> Option<u32>;
pub fn shard_for_region(region_index: u32, region_count: u32, key: &str, shard_count: u32) -> ShardId;
```

### Global vs. region-scoped keys

`shard_for` is **region-agnostic**: a key maps to one shard worldwide, so it's
globally coordinated (one lock everywhere) — put the leader near demand with
*leader affinity*. `shard_for_region` maps `(region, key)` into the band of
shards homed in that region for a geographically-close leader, but that makes the
key **region-scoped** (the same key in two regions is two different shards). Use
it only for region-local data; never for a key that must be globally consistent.

`shard_count` is **not** defined here — it's cluster configuration owned by the
brain (`ClusterConfig`) and passed in. It is fixed for the cluster's life, which
is what keeps `key → shard` stable while the node count scales.

## The hash is frozen

`fnv1a` is FNV-1a (32-bit). **Changing it remaps every key in every running
cluster** — a full data migration, not a code change. The `golden_vectors` test
pins its output (e.g. `shard_for("orders", 8) == 4`); a failure there means the
mapping moved and you must not ship it.

## Consumers

| Crate | Uses it for |
|-------|-------------|
| `fiducia-node` | route a committed command to its shard's Raft group |
| `fiducia-load-balance` | route a client request to the owning shard's leader |
| `fiducia-brain` | resolve `GET /v1/route?key=…` and place shards |

## Used as a dependency

Local development (all repos checked out side by side) uses a **path** dep:

```toml
fiducia-routing = { path = "../fiducia-routing.rs" }
```

In CI / in-pod builds (each service repo is built in isolation), switch to a
**git** dep so cargo fetches it:

```toml
fiducia-routing = { git = "https://github.com/fiducia-cloud/fiducia-routing.rs", tag = "v0.1.0" }
```

Pin a tag so a routing change is a deliberate, reviewed version bump across every
consumer — never an accidental drift.

## Test

```bash
cargo test   # determinism, bounds, and the golden vectors that freeze the hash
```
