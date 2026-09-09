# Fiducia routing replay peer contract

Canonical repository instructions: <https://github.com/ORESoftware/my-ai/blob/main/AGENTS.md>.

The TypeSpec and JSON Schema Draft 2020-12 files are independently authored,
equal authorities. Neither source is generated from the other; the official
emitter output is comparison evidence only.

This contract fixes the actual JSON-lines boundary exposed by
`formal/model.py --json-stdin`: hash, shard, organization scope, and regional
or global route requests plus their typed result envelopes. It therefore keeps
Rust and non-Rust replay adapters aligned without replacing the bounded model's
routing, band-containment, fallback, injectivity, and coordinator-placement
invariants.

CI pins `ORESoftware/typespec-json-schema-validator` to an immutable commit and
reruns on formal-model, Rust routing, or contract changes.
