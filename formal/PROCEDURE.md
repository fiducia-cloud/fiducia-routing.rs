# Formal-methods procedure: frozen routing contract

This procedure protects the key-to-shard contract shared by the Fiducia load balancer, brain, node, and operator tooling. Routing drift can split one logical key across different Raft groups, strand coordinator state, or break organization isolation, so routing changes are treated as data migrations rather than ordinary refactors.

## Claim boundary

`formal/model.py` is a finite executable abstraction of the pure functions in `src/lib.rs`. It exhaustively checks the bounded key, organization, region, and shard-count domains in `formal/fm.toml`. Rust unit and golden tests remain the production refinement gate. The model does not prove network delivery, Raft safety, placement convergence, or callers that bypass this crate.

## Model-to-code correspondence

| Model concept | Production surface |
|---|---|
| 32-bit FNV-1a, wrapping multiplication | `fnv1a` |
| region-agnostic placement | `shard_for` |
| organization framing | `org_scoped_key`, `org_scope_prefix` |
| contiguous regional bands and last-band remainder | `shard_for_region` |
| global/regional dispatch and safe fallback | `route_shard`, `KeyScope` |
| singleton coordinator placement | `LOCK_COORDINATION_KEY`, `SERVICE_DISCOVERY_KEY` |

## Required invariants

1. Every returned shard is within `[0, shard_count)`.
2. Global keys ignore all region input, including malformed and unknown values.
3. Regional keys stay inside the selected band when banding is possible.
4. Empty region lists or fewer shards than regions degrade to the globally convergent route.
5. Bounded `(organization, key)` pairs remain injective after delimiter framing.
6. Reserved coordinator keys and their checked production placements remain frozen.

## Change procedure

1. Classify the change. Altering FNV constants, byte encoding, scope framing, reserved keys, or golden placements is a migration.
2. Update the model and production code in the same PR. Never silently weaken or delete an invariant.
3. Add the smallest distinguishing counterexample to both the Python domain and Rust tests.
4. Run:

   ```bash
   python3 formal/model.py
   printf '%s\n' '{"op":"route","scope":"global","key":"orders/42","region":"aws","regions":["gcp","aws","hetzner"],"shard_count":16}' \
     | python3 formal/model.py --json-stdin
   cargo test --locked --all-targets
   ```

5. Confirm that no production routing branch is omitted from the abstraction.
6. For an intentional remap, publish the migration, dual-read/reindex, rollback, and cross-component rollout order before changing frozen vectors.

## JSON-lines adapter

`python3 formal/model.py --json-stdin` accepts one version-1 request per line and emits one canonical JSON result per line. Supported operations are `hash`, `shard`, `scope`, and `route`. Future Rust replay adapters must preserve these observable query semantics.

## Failure handling

Any counterexample is a hard failure. Do not update a golden value merely to make CI pass; treat it as evidence of a routing migration or model/code drift.

## Explicitly out of scope

This procedure does not claim liveness, balanced distribution, cryptographic hashing, stable placement when `shard_count` changes, or end-to-end agreement by independently reimplemented clients.
