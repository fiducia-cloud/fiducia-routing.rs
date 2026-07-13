# fiducia-routing — Architecture

This document explains what the `fiducia-routing` crate is, why it exists, how
its routing model works, and how it fits into the fiducia.cloud platform. It is
grounded in the actual code; file paths are cited throughout. For a quick API
reference, see `README.md`.

## 1. What this crate is

`fiducia-routing` is **not a service that routes traffic**. It is a small,
dependency-light **shared library** (plus one operator CLI binary) that answers
exactly one question, deterministically, for every component in the platform:

> **Given a key (and optionally a region), which shard owns it?**

A shard here is one independent Raft group in the sharded multi-Raft data plane
(`pub type ShardId = u32;` — `src/lib.rs`). The entire crate is a single module,
`src/lib.rs` (~260 lines of code plus an extensive test suite), and one binary,
`src/bin/fiducia-region.rs`. It has exactly one dependency
(`fiducia-interfaces`, the generated shared-types crate — `Cargo.toml`), and no
async runtime, no network client, no database driver. It talks to **nothing**:
no NATS, no Cockroach/Postgres, no HTTP. That is deliberate — pure functions
are the only way to guarantee every consumer computes the identical mapping.

### Why it exists: the split-brain-by-arithmetic problem

The fiducia data plane is sharded multi-Raft: there is no single leader; each
shard has its own leader spread across the nodes. Several independent
components must all agree on `key → shard`:

- the **load balancer** must forward a request to the leader of the key's shard;
- the **node** must apply a committed command in the key's shard's state machine;
- the **brain** (control plane) must answer `GET /v1/route?key=…` and place
  shards.

If any two of them computed the hash even slightly differently (different hash
function, different byte order, different modulo), the cluster would split its
brain: the LB would route a request to shard A while the data plane stored the
key in shard B, and locks would silently land in the wrong Raft group. The fix
is structural rather than procedural: **centralize the mapping in one crate and
forbid reimplementation** (`src/lib.rs` module docs, `README.md` "Why this
exists"). Even the JS edge worker is told, in `fiducia-edge/README.md`, that if
it ever needs `key → shard` it must compile *this crate* to WASM rather than
port the hash to JavaScript.

## 2. Position in the fiducia platform

The public request path is a three-tier pipeline; this crate is the arithmetic
used at the middle and bottom tiers:

```
client
  └─ fiducia-edge        (Cloudflare Worker, tier 1)   — picks a REGION (geo + health),
  │                                                      auth, rate limit; shard-agnostic
  └─ fiducia-load-balance (regional LB, tier 2)        — picks a NODE: extracts the
  │                                                      routing key, calls this crate's
  │                                                      shard_for(), forwards to that
  │                                                      shard's leader
  └─ fiducia-node        (data plane)                  — applies the command in the shard's
                                                         Raft group, using the SAME crate
                                                         to route committed commands
```

So versus the siblings named in the platform:

- **`fiducia-edge`** decides *which region* — it deliberately does **not** know
  about shards (see the "Two-tier routing" table in `fiducia-edge/README.md`).
- **`fiducia-load-balance`** decides *which node* — it re-exports this crate's
  primitives verbatim (`fiducia-load-balance.rs/src/routing.rs`:
  `pub use fiducia_routing::{shard_for, ShardId, LOCK_COORDINATION_KEY,
  SERVICE_DISCOVERY_KEY}`) and layers request→key extraction plus a
  stale-tolerant `shard → leader` cache on top.
- **`fiducia-routing` (this crate)** defines *which shard* — the frozen hash,
  the modulo, the region bands, and the reserved coordination keys. It is the
  layer both of the above (and the node and brain) share.

Confirmed consumers in the workspace (all via a path dep locally, a pinned git
tag in CI — `README.md` "Used as a dependency"):

| Consumer | Where | What it uses |
|---|---|---|
| `fiducia-node.rs` | `src/consensus.rs`, `src/state.rs`, `src/persist.rs`, `src/transport.rs` | `shard_for` for committed-command routing; `LOCK_COORDINATION_KEY` / `SERVICE_DISCOVERY_KEY` as state-machine domains; `ShardId` everywhere |
| `fiducia-load-balance.rs` | `src/routing.rs` | `shard_for`, `ShardId`, both coordination keys — the LB's key→shard step |
| `fiducia-brain.rs` | `src/config.rs`, `src/model.rs` | `shard_for` to resolve `GET /v1/route?key=…`; `ShardId` in the placement model |

Note what is *not* here: NATS JetStream messaging, leases, and fencing live in
other components (fiducia-node is the authority/lease/fencing layer;
Cockroach/PG holds state). This crate's contribution to those invariants is
indirect but foundational: fencing and leases are only meaningful if everyone
agrees which Raft group arbitrates a given key — and this crate is what makes
them agree.

## 3. The routing model

### 3.1 The frozen hash

```rust
pub fn fnv1a(s: &str) -> u32   // src/lib.rs
```

FNV-1a, 32-bit, over the key's UTF-8 bytes, with the standard offset basis
`0x811c_9dc5` and prime `0x0100_0193`. Chosen because it is tiny,
dependency-free, well-distributed for short string keys, and — critically —
byte-identical across processes and architectures.

**The hash is frozen.** Changing the constants, the byte order, or the
algorithm remaps every key in every running cluster, which is a full data
migration, not a code change. This is enforced mechanically by the
`golden_vectors` test in `src/lib.rs`, which pins concrete outputs
(`shard_for("orders", 8) == 4`, `fnv1a("") == 0x811c_9dc5`, etc.). A failure
there means "the mapping moved; you must not ship this."

### 3.2 Global (region-agnostic) sharding

```rust
pub fn shard_for(key: &str, shard_count: u32) -> ShardId   // fnv1a(key) % shard_count
```

One key → one shard **worldwide**. This is the mapping for anything that must
be globally coordinated (e.g. one lock everywhere). Proximity for global keys
is achieved not by changing the mapping but by *leader affinity* — the control
plane placing that shard's leader near demand.

`shard_count` is intentionally **not defined in this crate**. It is cluster
configuration owned by the brain (`fiducia-brain`'s `ClusterConfig`) and passed
in by every caller. It is fixed for the life of the cluster, which is exactly
what keeps `key → shard` stable while the *node* count scales — shards move
between nodes; keys never move between shards. `shard_for` panics on
`shard_count == 0` (a modulo-by-zero configuration bug worth failing loudly on).

### 3.3 Region-aware (banded) sharding

```rust
pub fn shard_for_region(region_index, region_count, key, shard_count) -> ShardId
pub fn shard_for_customer_region(region: Region, key, shard_count) -> ShardId
```

For region-local data, the shard space `[0, shard_count)` is split into
`region_count` contiguous **bands**; the region picks the band, the key picks
the shard within it: `base + (fnv1a(key) % band_size)`. The last band absorbs
the division remainder. With 3 regions and 256 shards the bands are
`[0,85) [85,170) [170,256)` (pinned by the
`customer_region_and_key_are_the_stable_routing_tuple` test). Shards in a
region's band are homed in that region, so the owning leader is geographically
close to the client.

The explicit trade-off, documented on `shard_for_region` itself: this makes the
key **region-scoped**. The same key in two regions is two different shards —
verified by the `same_key_different_region_is_geographically_local` test. It
must only be used for region-local data, never for a key that needs global
consistency.

Defensive behavior: `region_index` out of range **clamps to the last band**
rather than panicking (`ri.min(region_count - 1)`), so routing is always valid;
but `region_count == 0` or `shard_count < region_count` panics ("need at least
one shard per region").

### 3.4 The `Region` type

`Region` (`src/lib.rs`) is the customer-facing region enum:
`UsCentral1 | UsEast1 | EuCentral`, with

- `code()` — the stable API value (`"us-central1"`, `"us-east-1"`,
  `"eu-central"`);
- `cluster_name()` — the current backing cluster (`"gcp"`, `"aws"`,
  `"hetzner"`), decoupling the customer-facing name from the physical cluster;
- `index()` — its band index, which must stay aligned with the cluster order in
  `fiducia-infra/topology.toml` (`Region::ALL` order = topology order);
- `parse()` — tolerant parsing (trims, lowercases, accepts aliases like
  `"aws"`, `"eu-central-1"`);
- `nearest_to(lat, lon)` — nearest region by haversine distance to hard-coded
  approximate datacenter coordinates (`approximate_coordinates`,
  `haversine_km` at the bottom of `src/lib.rs`).

A deliberate design rule (README and `shard_for_customer_region` docs): region
is **explicit API input from the customer, never inferred from client IP**. IPs
change under a customer; the selected region is stable, so the routing tuple
`(region, key)` is stable.

### 3.5 Region resolution with graceful degradation

```rust
pub const DEFAULT_REGION_INDEX: u32 = 0;                     // topology primary
pub fn region_index(region, regions) -> Option<u32>;          // unknown → None
pub fn region_index_or(region, regions, default) -> u32;      // unknown → default
```

An unrecognized or empty `X-Fiducia-Region` header is **not an error**: it
degrades to the default region (index 0, the first cluster in `topology.toml`
order). Combined with the band clamp in `shard_for_region`, a request with any
region string — valid, garbage, or empty — always gets a valid shard.

### 3.6 `route_shard` and `KeyScope` — the footgun guard

```rust
pub enum KeyScope { Global, Regional }
pub fn route_shard(scope, key, region, regions, shard_count) -> ShardId
```

This is the single recommended entry point. The insight it encodes: **scope is
a property of the key/operation, not of the request's region header.** The
failure it exists to prevent is a *global* key being accidentally
region-sharded — two clients in different regions acquiring what they believe
is "the" lock but on two different shards, i.e. two locks. `route_shard`
makes that impossible by construction:

- `KeyScope::Global` → `shard_for(key, n)` — the region argument is **ignored
  entirely**, so every client converges on the same shard (test:
  `global_keys_ignore_region_entirely`);
- `KeyScope::Regional` → resolve the region with
  `region_index_or(…, DEFAULT_REGION_INDEX)` then `shard_for_region` (test:
  `regional_keys_route_into_their_region_band`).

### 3.7 Reserved coordination keys

Two reserved routing keys are defined here — in the shared crate — precisely so
the node, LB, and brain cannot disagree on which shard coordinates them. Both
start with a NUL byte (`\u{0}…`) so no real user key can ever collide with
them (asserted in tests).

**`LOCK_COORDINATION_KEY` / `lock_coordination_shard(n)`** — *all* lock and
semaphore state lives under one key, i.e. one shard. Rationale (doc comment in
`src/lib.rs`): multi-key **union** locks must be granted atomically and
conflict-checked across every member key, which requires one state machine to
see all of them together. So locks are *not* sharded by their user key; every
`/v1/locks/*` and `/v1/semaphores/*` operation routes to the single coordinator
shard (the live-mutex single-broker model, made HA by Raft). The LB honors this
in `fiducia-load-balance.rs/src/routing.rs` (any `["v1","locks",..]` or
`["v1","semaphores",..]` path → `LOCK_COORDINATION_KEY`), and the node uses the
same constant as its `LOCK_DOMAIN` (`fiducia-node.rs/src/state.rs`). If the LB
routed a single-key acquire on `B` by its own key while a composite lock on
`[A, B]` lived on the coordinator shard, the conflict would be missed — this
constant is what prevents that.

**`SERVICE_DISCOVERY_KEY` / `service_discovery_shard(n)`** — all
service-discovery state lives under one registry shard, so
`GET /v1/services` (a global list) is linearizable without a scatter-gather
read across every shard leader. Individual service lookups route through the
same shard so LB and node stay in lockstep (node's `SERVICE_DOMAIN`,
`fiducia-node.rs/src/state.rs`).

## 4. Request/data flow (end to end)

For a typical data-plane request, the crate's functions are exercised at two
independent points that must agree:

1. **Client → edge.** `fiducia-edge` (Cloudflare Worker) authenticates, rate
   limits, and picks the nearest healthy *region's* LB. No shard math.
2. **Edge → LB.** `fiducia-load-balance` extracts the routing key from the
   request shape (`routing_key(uri)` in its `src/routing.rs`: `?key=` for KV,
   path segments for rate-limit/cron/elections, the reserved constants for
   locks/semaphores/services, `None` for health/status/list endpoints), then
   computes `shard_for(key, shard_count)` **with this crate** and forwards to
   its cached leader for that shard (stale-tolerant cache, `NotLeader`/307
   backstop, refreshed from brain's `/v1/placement`).
3. **LB → node.** The node applies the committed command to the shard computed
   by `Command::routing_key` + `fiducia_routing::shard_for(key,
   self.config.shard_count)` (`fiducia-node.rs/src/consensus.rs`). Because steps
   2 and 3 call the same function from the same pinned crate version with the
   same `shard_count` from brain's `ClusterConfig`, they cannot diverge.
4. **Control plane.** `fiducia-brain` answers `GET /v1/route?key=…` with the
   same `shard_for` (`fiducia-brain.rs/src/config.rs`) and places shard
   replicas/leaders (leader affinity for global keys; region-band homing for
   regional shards).

## 5. Key invariants and why they hold

1. **One mapping, everywhere.** Enforced by architecture: the mapping exists in
   exactly one crate; consumers re-export rather than reimplement; the edge is
   instructed to use WASM builds of this crate if it ever needs the hash.
2. **The hash never changes silently.** Enforced by the `golden_vectors` test
   pinning concrete `shard_for`/`fnv1a` outputs; CI (`.github/workflows/ci.yml`)
   runs `cargo test --locked` on every push/PR, and consumers pin a reviewed
   git tag, so a routing change is a deliberate version bump across every
   consumer, never drift.
3. **`key → shard` is stable for the cluster's life.** Holds because
   `shard_count` is fixed cluster config owned by brain; scaling changes node
   count and shard *placement*, never the modulo.
4. **A global key can never be region-split.** Holds by construction in
   `route_shard`: `KeyScope::Global` ignores the region argument entirely.
5. **All locks/semaphores conflict-check in one state machine.** Holds because
   every component routes them via the shared `LOCK_COORDINATION_KEY` constant
   rather than the user key.
6. **Reserved keys cannot collide with user keys.** Leading NUL byte; asserted
   in tests.
7. **Routing is total.** Any region string resolves (unknown → default region;
   out-of-range index → clamped to last band); every returned shard is
   `< shard_count` (bounds asserted across the test suite). The only panics are
   loud configuration bugs: `shard_count == 0`, `region_count == 0`,
   `shard_count < region_count`.
8. **Region is API input, not IP inference.** `Region` values are explicit and
   stable; `nearest_to(lat/lon)` exists for tooling/suggestions, not for
   routing decisions made from connection metadata.

## 6. Module map

```
src/
├── lib.rs                # the entire library:
│                         #   fnv1a (frozen hash), ShardId, shard_for
│                         #   LOCK_COORDINATION_KEY / lock_coordination_shard
│                         #   SERVICE_DISCOVERY_KEY / service_discovery_shard
│                         #   Region (codes, cluster names, parse, nearest_to)
│                         #   shard_for_region / shard_for_customer_region
│                         #   DEFAULT_REGION_INDEX, region_index[_or]
│                         #   KeyScope + route_shard (the single entry point)
│                         #   haversine_km (private helper)
│                         #   #[cfg(test)] suite incl. golden_vectors
└── bin/
    └── fiducia-region.rs # operator CLI (see §7)
```

Supporting layout:

- `scripts/with-flags2env.sh` — bridges CLI flags to `FIDUCIA_*` env vars via
  the pinned `flags2env` parser and the `.cli-flags.toml` schema, then execs
  the given command.
- `.cli-flags.toml` — flag→env schema for the CLI (`--region` →
  `FIDUCIA_REGION`, `--shards` → `FIDUCIA_SHARD_COUNT`, etc.); audited in CI.
- `vendor/flags-2-env/` — git submodule: the pinned flag parser (C library +
  CLI), built with `make -C vendor/flags-2-env all`.
- `shell`, `.envrc`, `.nix/` — reproducible dev shell (Nix flake with the Rust
  toolchain; entered via direnv or `./shell`).
- `Dockerfile`, `.github/workflows/` — build/CI/deploy (see §7).
- `tmp/worktrees/` — local scratch; not part of the crate.

## 7. The `fiducia-region` CLI and error handling

`src/bin/fiducia-region.rs` is a thin operator tool over the library, for
inspecting and debugging region-aware routing from the shell:

- `--list` — print regions and their backing clusters (`us-east-1  cluster=aws`);
- `--lat/--lon` — resolve the nearest region to coordinates
  (`Region::nearest_to`);
- `--region <code> --key <key> [--shards <n>]` — print the shard
  (`shard_for_customer_region`); `--shards` defaults to 256.

Configuration comes from flags or the equivalent `FIDUCIA_*` env vars (set by
`scripts/with-flags2env.sh`), with flags taking precedence since they are
parsed after env. Argument parsing is hand-rolled (no clap): `main` returns
`Result<(), String>`, so every error is a short message with a hint ("unknown
region 'x'; run --list", "--lat and --lon must be provided together", "--key
needs --region or --lat/--lon") and a nonzero exit. Zero is rejected for
`--shards` before it could reach the library's panic. The library itself uses
`Option`/fallback for expected conditions (unknown region) and `assert!` only
for configuration impossibilities — consistent with "degrade for client input,
fail loudly for operator error."

## 8. Build, CI, and deployment

- **Toolchain**: pinned stable channel with rustfmt + clippy
  (`rust-toolchain.toml`); Nix dev shell for reproducible local environments.
- **Reproducibility**: `Cargo.lock` committed; every build (CI, Docker, docs'
  suggested commands) uses `--locked`. The single dependency,
  `fiducia-interfaces`, is a path dep locally
  (`../fiducia-interfaces/generated/rust`, `Cargo.toml`) and is pinned to the
  full 40-char commit `487e470c…` in CI and Docker; the Dockerfile fetches that
  exact object and verifies both `FETCH_HEAD` and the detached `HEAD` equal the
  declared commit, rejecting branches/tags/short hashes. The
  `generated_interfaces_are_importable` test keeps the interface crate wired.
- **CI** (`.github/workflows/ci.yml`): `cargo fmt --check`, locked all-target
  clippy with `-D warnings`, locked tests (including the golden vectors), and
  pinned `cargo-audit`.
- **CLI-flag audit** (`cli-flags.yml`): re-audits `.cli-flags.toml` with the
  pinned `flags2env` submodule whenever the schema/scripts/submodule change.
- **Container** (`Dockerfile`, `docker.yml`): two-stage build — pinned
  `rust:1.97.0-slim-bookworm` (by digest) builds and strips `fiducia-region`;
  the runtime is distroless `cc-debian12:nonroot` (by digest) running as UID
  65532 with the CLI as entrypoint. Pushed to
  `ghcr.io/fiducia-cloud/fiducia-routing` (`latest` + commit SHA) on every push
  to `main`.
- **Deploy** (`deploy-test.yml`): app repos deploy only to the TEST
  environment from their own CI (`fiducia-monorepo` owns PROD). The workflow
  rolls `deployment/fiducia-routing` in the `fiducia-test` namespace to the
  commit-SHA image and waits for rollout, gated on the `KUBE_CONFIG_TEST`
  secret. Note this deploys the *CLI image*; the library itself ships to
  production only as a pinned dependency inside `fiducia-node`,
  `fiducia-load-balance`, and `fiducia-brain`.
- **Consuming the crate**: local workspace uses `path = "../fiducia-routing.rs"`;
  isolated CI/in-pod builds use a git dep pinned to a tag, so any routing
  change is a deliberate, reviewed version bump in every consumer.

## 9. What to remember when changing this crate

- Never change `fnv1a` or the band arithmetic in `shard_for_region`; if a
  golden-vector test fails, the mapping moved and the change must not ship.
- Keep `Region::ALL` order aligned with `fiducia-infra/topology.toml` and the
  generated edge region list — the band index *is* the topology order.
- Adding a Region variant changes `region_count`, which **re-bands every
  regional key**: treat it as a data migration, not an enum addition.
- New reserved coordination keys must start with `\u{0}` and be defined here,
  not in a consumer.
- Version-bump consumers deliberately (pinned tag), and update the
  `INTERFACES_REF` in `Dockerfile`, `docker.yml`, and `ci.yml` together when
  moving the interfaces pin.
