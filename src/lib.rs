//! Shared key → shard routing for fiducia.cloud.
//!
//! Every component must map a key to the same shard, or the cluster splits its
//! brain: the load balancer would route a request to one shard while the data
//! plane stored it in another. This crate is the **single source of truth** for
//! that mapping so it physically cannot drift —
//! [`fiducia-node`](https://github.com/fiducia-cloud/fiducia-node.rs),
//! [`fiducia-brain`](https://github.com/fiducia-cloud/fiducia-brain.rs), and
//! [`fiducia-load-balance`](https://github.com/fiducia-cloud/fiducia-load-balance.rs)
//! all depend on it instead of carrying their own copy.
//!
//! Two things only:
//!   * [`fnv1a`] — the hash. **Frozen.** Changing it remaps every key in the
//!     cluster (a full data migration), so the golden tests below pin its output.
//!   * [`shard_for`] — `hash(key) % shard_count`.
//!
//! `shard_count` is *not* defined here — it's cluster configuration owned by the
//! brain ([`fiducia-brain`]'s `ClusterConfig`), passed in by the caller. It is
//! fixed for the cluster's life, which is what makes `key → shard` stable while
//! the node count scales.

/// Identifier of a shard (one independent Raft group).
pub type ShardId = u32;

/// Map a key to its shard.
///
/// # Panics
/// Panics if `shard_count == 0` — a cluster always has at least one shard, and a
/// modulo by zero is a configuration bug worth failing loudly on.
#[inline]
pub fn shard_for(key: &str, shard_count: u32) -> ShardId {
    assert!(shard_count > 0, "shard_count must be > 0");
    fnv1a(key) % shard_count
}

/// Region-aware sharding: map `(region, key)` into the band of shards homed in
/// that region, so the owning shard's leader is geographically close to the
/// client. The shard space is split into `region_count` contiguous bands; the
/// last band absorbs any remainder.
///
/// **Important — this makes the key REGION-SCOPED.** The same key in two
/// different regions maps to two *different* shards, so it is NOT globally
/// coordinated. Use this only for region-local data (low-latency, region-pinned).
/// For a key that must be globally consistent (one lock worldwide) use
/// [`shard_for`] (region-agnostic) and rely on *leader affinity* to place that
/// shard's leader near demand instead.
///
/// # Panics
/// Panics if `region_count == 0` or `shard_count < region_count`.
#[inline]
pub fn shard_for_region(region_index: u32, region_count: u32, key: &str, shard_count: u32) -> ShardId {
    assert!(region_count > 0, "region_count must be > 0");
    assert!(shard_count >= region_count, "need at least one shard per region");
    let ri = region_index.min(region_count - 1); // clamp out-of-range to last band
    let band = shard_count / region_count; // shards per region (floor)
    let base = ri * band;
    // The last region owns the remainder shards too.
    let size = if ri == region_count - 1 { shard_count - base } else { band };
    base + (fnv1a(key) % size)
}

/// Conventional default region: index 0 — the first region in `topology.toml`
/// order (the cluster's primary). Used when a client's region is unrecognized.
pub const DEFAULT_REGION_INDEX: u32 = 0;

/// Resolve a region name to its index in an ordered region list (the order is
/// the cluster order in `topology.toml`). Returns `None` for an unknown region.
#[inline]
pub fn region_index(region: &str, regions: &[&str]) -> Option<u32> {
    regions.iter().position(|r| *r == region).map(|i| i as u32)
}

/// Resolve a (possibly client-supplied, possibly unknown or empty) region name to
/// an index, falling back to `default_index` when it isn't recognized — so a
/// typo'd or unset `X-Fiducia-Region` degrades to a sensible **default region**
/// instead of erroring. Pair with [`DEFAULT_REGION_INDEX`] for the primary.
#[inline]
pub fn region_index_or(region: &str, regions: &[&str], default_index: u32) -> u32 {
    region_index(region, regions).unwrap_or(default_index)
}

/// FNV-1a (32-bit) — small, dependency-free, well-distributed, and identical
/// across processes and architectures. **Do not change** the constants or the
/// byte order; the golden vectors in the tests exist to stop exactly that.
#[inline]
pub fn fnv1a(s: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5; // FNV offset basis
    for b in s.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193); // FNV prime
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_bounded() {
        for key in ["orders", "checkout", "orders/checkout", "api", "cleanup", ""] {
            for n in [1u32, 4, 16, 256, 1024] {
                assert!(shard_for(key, n) < n);
                assert_eq!(shard_for(key, n), shard_for(key, n));
            }
        }
    }

    #[test]
    fn region_index_lookup() {
        let regions = ["gcp", "aws", "hetzner"];
        assert_eq!(region_index("gcp", &regions), Some(0));
        assert_eq!(region_index("hetzner", &regions), Some(2));
        assert_eq!(region_index("azure", &regions), None);
    }

    #[test]
    fn unknown_region_falls_back_to_default() {
        let regions = ["gcp", "aws", "hetzner"];
        assert_eq!(region_index_or("aws", &regions, DEFAULT_REGION_INDEX), 1); // known wins
        assert_eq!(region_index_or("azure", &regions, DEFAULT_REGION_INDEX), 0); // unknown -> default
        assert_eq!(region_index_or("", &regions, 2), 2); // empty -> caller's default
        // The resolved index is always a valid band for shard_for_region.
        let n = 12;
        assert!(shard_for_region(region_index_or("nope", &regions, DEFAULT_REGION_INDEX), 3, "k", n) < n);
    }

    #[test]
    fn region_sharding_lands_in_the_region_band() {
        // 3 regions, 12 shards -> bands [0,4) [4,8) [8,12).
        let (rc, n) = (3u32, 12u32);
        for key in ["orders/checkout", "api", "user-42", "x"] {
            assert!((0..4).contains(&shard_for_region(0, rc, key, n)), "region 0 band");
            assert!((4..8).contains(&shard_for_region(1, rc, key, n)), "region 1 band");
            assert!((8..12).contains(&shard_for_region(2, rc, key, n)), "region 2 band");
        }
    }

    #[test]
    fn same_key_different_region_is_geographically_local() {
        // The whole point: a client routes to a shard in ITS region (closer
        // leader). The same key in different regions therefore lands on
        // different, region-local shards.
        let (rc, n) = (3u32, 12u32);
        let g = shard_for_region(0, rc, "orders/checkout", n);
        let a = shard_for_region(1, rc, "orders/checkout", n);
        let h = shard_for_region(2, rc, "orders/checkout", n);
        assert_ne!(g, a);
        assert_ne!(a, h);
        assert_ne!(g, h);
    }

    #[test]
    fn region_sharding_bounded_and_remainder_in_last_band() {
        // 3 regions, 16 shards -> bands of 5,5,6 (last absorbs remainder).
        let (rc, n) = (3u32, 16u32);
        for key in ["a", "bb", "ccc", "orders", "lock-9"] {
            assert!(shard_for_region(2, rc, key, n) < n);
            assert!(shard_for_region(2, rc, key, n) >= 10, "last band starts at 10");
        }
        // Out-of-range region index clamps to the last band rather than panicking.
        assert!(shard_for_region(99, rc, "x", n) >= 10);
    }

    /// Golden vectors — these pin the hash. If one of these ever changes, the
    /// mapping changed and every key in every running cluster just moved. Treat
    /// a failure here as "you must not ship this".
    #[test]
    fn golden_vectors() {
        // Locked from the running node/LB/brain (shard_count = 8).
        assert_eq!(shard_for("checkout", 8), 1);
        assert_eq!(shard_for("orders", 8), 4);
        assert_eq!(shard_for("orders/checkout", 8), 5);

        // Raw hash, independent of shard_count.
        assert_eq!(fnv1a(""), 0x811c_9dc5);
    }

    #[test]
    #[should_panic]
    fn zero_shard_count_panics() {
        let _ = shard_for("x", 0);
    }
}
