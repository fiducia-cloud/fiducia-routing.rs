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
//! The core contract:
//!   * [`fnv1a`] — the hash. **Frozen.** Changing it remaps every key in the
//!     cluster (a full data migration), so the golden tests below pin its output.
//!   * [`shard_for`] — `hash(key) % shard_count`.
//!   * [`org_scoped_key`] — the per-org namespacing (`\x01{org}\x01{key}`) the
//!     node applies before a key reaches the state machine. **Also frozen**
//!     (it's what stored keys look like), and part of routing because the
//!     scoped key is what gets hashed: the LB must scope before it shards.
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

/// Reserved routing key under which **all** lock + semaphore state lives.
///
/// Multi-key *union* locks must be atomic and conflict-checked across every
/// member key, which requires one state machine to see them together. So locks
/// and semaphores are **not** sharded by their own key — every lock/semaphore
/// operation routes to the single shard that owns this reserved key (the
/// live-mutex single-broker model, made HA by Raft). It is defined here, in the
/// shared routing crate, so the node (`Command::routing_key`), the load balancer,
/// and the brain cannot disagree on which shard coordinates locks.
///
/// The leading NUL keeps it from colliding with any real user key.
pub const LOCK_COORDINATION_KEY: &str = "\u{0}fiducia-lock-coordinator";

/// The shard that coordinates **all** locks/semaphores, for a given shard count.
/// The LB sends every `/v1/locks/*` and `/v1/semaphores/*` request to this
/// shard's leader; the node routes every lock/semaphore command here too.
#[inline]
pub fn lock_coordination_shard(shard_count: u32) -> ShardId {
    shard_for(LOCK_COORDINATION_KEY, shard_count)
}

/// Delimiter framing an org id inside an org-scoped key. `\u{1}` cannot appear
/// in a valid org id (the node rejects control characters), so a crafted caller
/// key can never escape its org's keyspace.
pub const ORG_SCOPE_DELIM: char = '\u{1}';

/// Namespace a caller-supplied key into an org's private keyspace:
/// `\u{1}{org}\u{1}{key}`.
///
/// This is part of the **routing contract**, not just node-internal storage:
/// the node commits an org's command under the scoped key, so the scoped key is
/// what gets hashed to a shard. Every component that predicts a shard for an
/// org-addressed request (the load balancer picking a leader, an operator tool
/// resolving a key) must hash `org_scoped_key(org, key)` — hashing the raw
/// caller key computes a different, wrong shard. Defined here so the node and
/// the LB physically cannot disagree on the format.
#[inline]
pub fn org_scoped_key(org_id: &str, key: &str) -> String {
    format!("{ORG_SCOPE_DELIM}{org_id}{ORG_SCOPE_DELIM}{key}")
}

/// The prefix every key in `org_id`'s keyspace carries — `org_scoped_key` of
/// the empty key. Strip it to recover the caller-facing key; a scoped key that
/// doesn't start with it belongs to a different org (the isolation filter).
#[inline]
pub fn org_scope_prefix(org_id: &str) -> String {
    org_scoped_key(org_id, "")
}

/// Reserved routing key under which **all** service-discovery state lives.
///
/// A list of service names is a global registry operation, not a single-service
/// lookup. Keeping discovery under one coordinator shard makes
/// `GET /v1/services` linearizable without a scatter-gather read across every
/// shard leader. Individual service lookups still return just that service's
/// live instances, but they route through this same registry shard so the load
/// balancer and node stay in lockstep.
pub const SERVICE_DISCOVERY_KEY: &str = "\u{0}fiducia-service-discovery";

/// The shard that coordinates service discovery for a given shard count.
#[inline]
pub fn service_discovery_shard(shard_count: u32) -> ShardId {
    shard_for(SERVICE_DISCOVERY_KEY, shard_count)
}

/// Customer-selectable region values.
///
/// These are the stable API values customers pass with their key. They map onto
/// the current cluster order from `fiducia-infra/topology.toml`; keep the order in
/// [`Region::ALL`] aligned with the generated edge region list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Region {
    UsCentral1,
    UsEast1,
    EuCentral,
}

impl Region {
    pub const ALL: [Region; 3] = [Region::UsCentral1, Region::UsEast1, Region::EuCentral];

    /// Stable customer-facing API value.
    pub const fn code(self) -> &'static str {
        match self {
            Region::UsCentral1 => "us-central1",
            Region::UsEast1 => "us-east-1",
            Region::EuCentral => "eu-central",
        }
    }

    /// Current backing cluster name for this customer-facing region.
    pub const fn cluster_name(self) -> &'static str {
        match self {
            Region::UsCentral1 => "gcp",
            Region::UsEast1 => "aws",
            Region::EuCentral => "hetzner",
        }
    }

    pub const fn index(self) -> u32 {
        match self {
            Region::UsCentral1 => 0,
            Region::UsEast1 => 1,
            Region::EuCentral => 2,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "us-central1" | "us-central" | "gcp" => Some(Region::UsCentral1),
            "us-east-1" | "us-east" | "aws" => Some(Region::UsEast1),
            "eu-central" | "eu-central-1" | "hetzner" => Some(Region::EuCentral),
            _ => None,
        }
    }

    pub fn nearest_to(latitude: f64, longitude: f64) -> Self {
        let mut best = Region::ALL[0];
        let mut best_distance = best.distance_km_to(latitude, longitude);
        for region in Region::ALL.iter().copied().skip(1) {
            let distance = region.distance_km_to(latitude, longitude);
            if distance < best_distance {
                best = region;
                best_distance = distance;
            }
        }
        best
    }

    pub fn distance_km_to(self, latitude: f64, longitude: f64) -> f64 {
        let (region_latitude, region_longitude) = self.approximate_coordinates();
        haversine_km(latitude, longitude, region_latitude, region_longitude)
    }

    fn approximate_coordinates(self) -> (f64, f64) {
        match self {
            Region::UsCentral1 => (41.2619, -95.8608),
            Region::UsEast1 => (38.13, -78.45),
            Region::EuCentral => (50.4761, 12.3700),
        }
    }
}

/// Customer-facing region-aware routing: map `(region, key)` to a shard.
///
/// This is intentionally independent of client IP. IPs can change underneath the
/// customer; the selected region is an explicit API input and therefore stable.
#[inline]
pub fn shard_for_customer_region(region: Region, key: &str, shard_count: u32) -> ShardId {
    shard_for_region(region.index(), Region::ALL.len() as u32, key, shard_count)
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
pub fn shard_for_region(
    region_index: u32,
    region_count: u32,
    key: &str,
    shard_count: u32,
) -> ShardId {
    assert!(region_count > 0, "region_count must be > 0");
    assert!(
        shard_count >= region_count,
        "need at least one shard per region"
    );
    let ri = region_index.min(region_count - 1); // clamp out-of-range to last band
    let band = shard_count / region_count; // shards per region (floor)
    let base = ri * band;
    // The last region owns the remainder shards too.
    let size = if ri == region_count - 1 {
        shard_count - base
    } else {
        band
    };
    base + (fnv1a(key) % size)
}

/// Conventional default region: index 0 — the first region in `topology.toml`
/// order (the cluster's primary). Used when a client's region is unrecognized.
pub const DEFAULT_REGION_INDEX: u32 = 0;

/// Resolve a region name to its index in an ordered region list (the order is
/// the cluster order in `topology.toml`). HTTP/header inputs commonly carry
/// harmless surrounding whitespace or case differences, so matching is ASCII
/// case-insensitive after trimming. Returns `None` for an unknown region.
#[inline]
pub fn region_index(region: &str, regions: &[&str]) -> Option<u32> {
    let region = region.trim();
    regions
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(region))
        .map(|i| i as u32)
}

/// Resolve a (possibly client-supplied, possibly unknown or empty) region name to
/// an index, falling back to `default_index` when it isn't recognized — so a
/// typo'd or unset `X-Fiducia-Region` degrades to a sensible **default region**
/// instead of erroring. Pair with [`DEFAULT_REGION_INDEX`] for the primary.
#[inline]
pub fn region_index_or(region: &str, regions: &[&str], default_index: u32) -> u32 {
    region_index(region, regions).unwrap_or(default_index)
}

/// Whether a key is coordinated globally or scoped to a region. **This is a
/// property of the key/operation, not of the request's region header.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyScope {
    /// One shard worldwide; region is ignored entirely (correctness). Use leader
    /// affinity to place that shard's leader near demand.
    Global,
    /// Region-local data: routed into the region's shard band for a nearby leader.
    Regional,
}

/// The single routing entry point — pick a key's shard, doing the right thing for
/// its scope so a *global* key can never be accidentally region-sharded:
///
/// * [`KeyScope::Global`] → [`shard_for`] — **region is ignored**, so every
///   client (any region, default or not) converges on the *same* shard.
/// * [`KeyScope::Regional`] → [`shard_for_region`] using the resolved region
///   (unknown ⇒ `DEFAULT_REGION_INDEX`); the region picks the band, the key picks
///   the shard within it.
#[inline]
pub fn route_shard(
    scope: KeyScope,
    key: &str,
    region: &str,
    regions: &[&str],
    shard_count: u32,
) -> ShardId {
    match scope {
        KeyScope::Global => shard_for(key, shard_count),
        // An empty region list (or fewer shards than regions) makes regional
        // banding impossible — degrade to GLOBAL routing rather than panic on a
        // request path. Global is the safe direction: every caller still
        // converges on the same shard; only leader locality is lost.
        KeyScope::Regional if regions.is_empty() || shard_count < regions.len() as u32 => {
            shard_for(key, shard_count)
        }
        KeyScope::Regional => {
            let ri = region_index_or(region, regions, DEFAULT_REGION_INDEX);
            shard_for_region(ri, regions.len() as u32, key, shard_count)
        }
    }
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let d_lat = (lat2 - lat1).to_radians();
    let d_lon = (lon2 - lon1).to_radians();
    let lat1 = lat1.to_radians();
    let lat2 = lat2.to_radians();
    let a = (d_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
    let c = 2.0 * a.min(1.0).sqrt().atan2((1.0 - a).max(0.0).sqrt());
    6_371.0 * c
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
        for key in [
            "orders",
            "checkout",
            "orders/checkout",
            "api",
            "cleanup",
            "",
        ] {
            for n in [1u32, 4, 16, 256, 1024] {
                assert!(shard_for(key, n) < n);
                assert_eq!(shard_for(key, n), shard_for(key, n));
            }
        }
    }

    #[test]
    fn lock_coordination_is_stable_and_shared() {
        // Every lock/semaphore op hashes to ONE coordinator shard, deterministically.
        for n in [1u32, 4, 16, 256, 1024] {
            let s = lock_coordination_shard(n);
            assert!(s < n);
            assert_eq!(s, shard_for(LOCK_COORDINATION_KEY, n));
            assert_eq!(s, lock_coordination_shard(n)); // deterministic
        }
        // The reserved key cannot be a real user key (leading NUL).
        assert!(LOCK_COORDINATION_KEY.starts_with('\u{0}'));
    }

    #[test]
    fn service_discovery_coordination_is_stable_and_shared() {
        for n in [1u32, 4, 16, 256, 1024] {
            let s = service_discovery_shard(n);
            assert!(s < n);
            assert_eq!(s, shard_for(SERVICE_DISCOVERY_KEY, n));
            assert_eq!(s, service_discovery_shard(n));
        }
        assert!(SERVICE_DISCOVERY_KEY.starts_with('\u{0}'));
        assert_ne!(SERVICE_DISCOVERY_KEY, LOCK_COORDINATION_KEY);
    }

    #[test]
    fn region_index_lookup() {
        let regions = ["gcp", "aws", "hetzner"];
        assert_eq!(region_index("gcp", &regions), Some(0));
        assert_eq!(region_index("hetzner", &regions), Some(2));
        assert_eq!(region_index(" AWS ", &regions), Some(1));
        assert_eq!(region_index("HeTzNeR", &regions), Some(2));
        assert_eq!(region_index("azure", &regions), None);
    }

    #[test]
    fn global_keys_ignore_region_entirely() {
        // The footgun guard: a Global key maps to the SAME shard no matter the
        // region — valid, invalid, or empty — so it can't split into two locks.
        let regions = ["gcp", "aws", "hetzner"];
        let n = 64;
        let base = route_shard(KeyScope::Global, "orders/checkout", "gcp", &regions, n);
        for region in ["aws", "hetzner", "azure", "", "garbage"] {
            assert_eq!(
                route_shard(KeyScope::Global, "orders/checkout", region, &regions, n),
                base
            );
        }
        // ...and it equals the plain region-agnostic hash.
        assert_eq!(base, shard_for("orders/checkout", n));
    }

    #[test]
    fn regional_keys_route_into_their_region_band() {
        let regions = ["gcp", "aws", "hetzner"]; // 3 regions
        let n = 12; // bands [0,4) [4,8) [8,12)
        assert!((4..8).contains(&route_shard(KeyScope::Regional, "k", "aws", &regions, n)));
        // unknown region -> default band [0,4)
        assert!((0..4).contains(&route_shard(KeyScope::Regional, "k", "nope", &regions, n)));
    }

    #[test]
    fn regional_routing_degrades_to_global_when_banding_is_impossible() {
        // An empty region list must not panic a request path — it falls back to
        // the region-agnostic hash (everyone still converges on one shard).
        let n = 12;
        assert_eq!(
            route_shard(KeyScope::Regional, "k", "aws", &[], n),
            shard_for("k", n)
        );
        // Fewer shards than regions likewise cannot band; same safe fallback.
        let many = ["a", "b", "c", "d", "e"];
        assert_eq!(
            route_shard(KeyScope::Regional, "k", "b", &many, 3),
            shard_for("k", 3)
        );
    }

    #[test]
    fn customer_region_and_key_are_the_stable_routing_tuple() {
        let shard_count = 256;
        let key = "orders/checkout";

        let central = shard_for_customer_region(Region::UsCentral1, key, shard_count);
        let east = shard_for_customer_region(Region::UsEast1, key, shard_count);
        let europe = shard_for_customer_region(Region::EuCentral, key, shard_count);

        assert_eq!(
            central,
            shard_for_customer_region(Region::UsCentral1, key, shard_count)
        );
        assert_eq!(
            east,
            shard_for_customer_region(Region::UsEast1, key, shard_count)
        );
        assert_eq!(
            europe,
            shard_for_customer_region(Region::EuCentral, key, shard_count)
        );

        assert!((0..85).contains(&central), "us-central1 band");
        assert!((85..170).contains(&east), "us-east-1 band");
        assert!((170..256).contains(&europe), "eu-central band");
        assert_ne!(central, east);
        assert_ne!(east, europe);
        assert_ne!(central, europe);
    }

    #[test]
    fn customer_region_values_parse_and_select_nearest_region() {
        assert_eq!(Region::parse("us-central1"), Some(Region::UsCentral1));
        assert_eq!(Region::parse("aws"), Some(Region::UsEast1));
        assert_eq!(Region::parse("hetzner"), Some(Region::EuCentral));
        assert_eq!(Region::parse("moon"), None);

        assert_eq!(Region::nearest_to(38.8977, -77.0365), Region::UsEast1);
        assert_eq!(Region::nearest_to(41.25, -95.9), Region::UsCentral1);
        assert_eq!(Region::nearest_to(50.1, 8.7), Region::EuCentral);
    }

    #[test]
    fn customer_region_aliases_are_case_and_space_tolerant() {
        assert_eq!(Region::parse(" US-CENTRAL "), Some(Region::UsCentral1));
        assert_eq!(Region::parse("AWS "), Some(Region::UsEast1));
        assert_eq!(Region::parse(" eu-central-1 "), Some(Region::EuCentral));
    }

    #[test]
    fn customer_region_wrapper_matches_explicit_region_band() {
        let shard_count = 257;
        for region in Region::ALL {
            assert_eq!(
                shard_for_customer_region(region, "sessions/user-42", shard_count),
                shard_for_region(
                    region.index(),
                    Region::ALL.len() as u32,
                    "sessions/user-42",
                    shard_count
                )
            );
        }
    }

    #[test]
    #[should_panic(expected = "need at least one shard per region")]
    fn region_sharding_requires_at_least_one_shard_per_region() {
        let _ = shard_for_region(0, 3, "too-small", 2);
    }

    #[test]
    fn unknown_region_falls_back_to_default() {
        let regions = ["gcp", "aws", "hetzner"];
        assert_eq!(region_index_or("aws", &regions, DEFAULT_REGION_INDEX), 1); // known wins
        assert_eq!(region_index_or("azure", &regions, DEFAULT_REGION_INDEX), 0); // unknown -> default
        assert_eq!(region_index_or("", &regions, 2), 2); // empty -> caller's default
                                                         // The resolved index is always a valid band for shard_for_region.
        let n = 12;
        assert!(
            shard_for_region(
                region_index_or("nope", &regions, DEFAULT_REGION_INDEX),
                3,
                "k",
                n
            ) < n
        );
    }

    #[test]
    fn region_sharding_lands_in_the_region_band() {
        // 3 regions, 12 shards -> bands [0,4) [4,8) [8,12).
        let (rc, n) = (3u32, 12u32);
        for key in ["orders/checkout", "api", "user-42", "x"] {
            assert!(
                (0..4).contains(&shard_for_region(0, rc, key, n)),
                "region 0 band"
            );
            assert!(
                (4..8).contains(&shard_for_region(1, rc, key, n)),
                "region 1 band"
            );
            assert!(
                (8..12).contains(&shard_for_region(2, rc, key, n)),
                "region 2 band"
            );
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
            assert!(
                shard_for_region(2, rc, key, n) >= 10,
                "last band starts at 10"
            );
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

    #[test]
    fn generated_interfaces_are_importable() {
        let request = fiducia_interfaces::LockAcquireManyRequest {
            keys: vec!["orders/42".to_string(), "inventory/sku-7".to_string()],
            holder: Some("worker-a".to_string()),
            ttl_ms: Some(30_000),
            wait: Some(false),
        };

        assert_eq!(request.keys.len(), 2);
        assert!(matches!(
            fiducia_interfaces::ProposeErrorReason::NotLeader,
            fiducia_interfaces::ProposeErrorReason::NotLeader
        ));
    }

    /// The published FNV-1a 32-bit reference vectors (Noll's test suite). These
    /// are the cross-language anchor: any other implementation of the routing
    /// hash (e.g. fiducia-e2e's JS conformance copy) must reproduce exactly
    /// these, or it is not hashing what the cluster hashes.
    #[test]
    fn fnv1a_matches_published_reference_vectors() {
        assert_eq!(fnv1a(""), 0x811c_9dc5);
        assert_eq!(fnv1a("a"), 0xe40c_292c);
        assert_eq!(fnv1a("b"), 0xe70c_2de5);
        assert_eq!(fnv1a("foobar"), 0xbf9c_f968);
        assert_eq!(fnv1a("hello"), 0x4f9f_2cab);
        assert_eq!(fnv1a("orders/checkout"), 0xd217_6c9d);
    }

    /// Pin the coordinator shards at the shard counts real deployments run
    /// (node default 16; the golden-vector cluster 8; the region default 256).
    /// If one of these moves — a changed reserved key string or hash — every
    /// running cluster's lock/discovery state is suddenly on the "wrong" shard.
    #[test]
    fn coordination_shards_are_pinned_at_deployed_shard_counts() {
        assert_eq!(lock_coordination_shard(16), 15);
        assert_eq!(lock_coordination_shard(8), 7);
        assert_eq!(lock_coordination_shard(256), 223);
        assert_eq!(service_discovery_shard(16), 9);
        assert_eq!(service_discovery_shard(8), 1);
        assert_eq!(service_discovery_shard(256), 233);
    }

    /// Region bands must tile the shard space exactly: contiguous, disjoint,
    /// covering [0, shard_count), with the last band absorbing the remainder —
    /// for ANY (region_count, shard_count), not just the 3-region examples.
    #[test]
    fn region_bands_partition_the_shard_space() {
        for rc in [1u32, 2, 3, 5, 7] {
            for n in [rc, rc + 1, rc * 2 + 1, 64, 257] {
                let band = n / rc;
                let mut covered = 0u32;
                for ri in 0..rc {
                    let base = ri * band;
                    let size = if ri == rc - 1 { n - base } else { band };
                    assert_eq!(base, covered, "bands must be contiguous (rc={rc}, n={n})");
                    covered += size;
                    // Sampled keys stay inside their region's band.
                    for key in ["orders/checkout", "a", "user-42", "🔒", "\u{0}x"] {
                        let shard = shard_for_region(ri, rc, key, n);
                        assert!(
                            (base..base + size).contains(&shard),
                            "key {key:?} escaped band {ri} (rc={rc}, n={n}): {shard}"
                        );
                    }
                }
                assert_eq!(covered, n, "bands must cover every shard (rc={rc}, n={n})");
            }
        }
    }

    /// Every shard must be reachable and the hash roughly uniform — a keyspace
    /// that starves a shard (or floods one) means hot leaders and wasted Raft
    /// groups. Loose bounds: this is a smoke alarm, not a chi-squared test.
    #[test]
    fn keys_spread_across_every_shard_roughly_uniformly() {
        let n = 16u32;
        let keys = 20_000u32;
        let mut counts = vec![0u32; n as usize];
        for i in 0..keys {
            counts[shard_for(&format!("tenant-{}/key-{i}", i % 97), n) as usize] += 1;
        }
        let expected = keys / n; // 1250
        for (shard, &count) in counts.iter().enumerate() {
            assert!(
                count > expected / 2 && count < expected * 2,
                "shard {shard} got {count} of {keys} keys (expected ≈{expected})"
            );
        }
    }

    /// Unicode, embedded NUL, and very long keys hash deterministically and in
    /// bounds — routing sees raw bytes, whatever the customer sends.
    #[test]
    fn unusual_keys_route_deterministically_and_in_bounds() {
        let long_key = "k".repeat(64 * 1024);
        for key in ["🔒/orders/结账", "a\u{0}b", "\u{0}", long_key.as_str()] {
            for n in [1u32, 16, 257] {
                let s = shard_for(key, n);
                assert!(s < n);
                assert_eq!(s, shard_for(key, n));
            }
        }
    }

    /// The customer-facing region values round-trip through `parse`, stay
    /// aligned with `index()`/`ALL` order, and the cluster order is pinned to
    /// `fiducia-infra/topology.toml` (gcp, aws, hetzner). Reordering `ALL`
    /// silently remaps every regional key — that must fail a test, not a
    /// customer.
    #[test]
    fn region_codes_round_trip_and_cluster_order_is_pinned() {
        let mut seen = std::collections::HashSet::new();
        for (position, region) in Region::ALL.iter().copied().enumerate() {
            assert_eq!(Region::parse(region.code()), Some(region));
            assert_eq!(Region::parse(region.cluster_name()), Some(region));
            assert_eq!(region.index() as usize, position);
            assert!(seen.insert(region.code()), "codes must be unique");
        }
        let clusters: Vec<&str> = Region::ALL.iter().map(|r| r.cluster_name()).collect();
        assert_eq!(clusters, ["gcp", "aws", "hetzner"], "topology.toml order");
    }

    /// The two regional entry points agree: the customer wrapper (typed Region)
    /// and the generic `route_shard` (region string + cluster list) must land on
    /// the same shard when the list is in topology order, or the LB and an
    /// operator tool could disagree about where a regional key lives.
    #[test]
    fn customer_wrapper_agrees_with_route_shard_over_cluster_list() {
        let regions = ["gcp", "aws", "hetzner"];
        for n in [3u32, 12, 64, 257] {
            for key in ["orders/checkout", "sessions/user-42", "x"] {
                for region in Region::ALL {
                    assert_eq!(
                        shard_for_customer_region(region, key, n),
                        route_shard(KeyScope::Regional, key, region.cluster_name(), &regions, n),
                        "wrapper vs route_shard drift (region={}, key={key}, n={n})",
                        region.code()
                    );
                }
            }
        }
    }

    /// The org-scoped key format is pinned: the node stores under it, so the
    /// shard it hashes to is part of the cluster's persistent layout. Changing
    /// the format remaps (and orphans) every org's data.
    #[test]
    fn org_scoped_key_format_is_pinned() {
        assert_eq!(
            org_scoped_key("org_1", "orders/checkout"),
            "\u{1}org_1\u{1}orders/checkout"
        );
        assert_eq!(org_scope_prefix("org_1"), "\u{1}org_1\u{1}");
        assert!(org_scoped_key("org_1", "k").starts_with(&org_scope_prefix("org_1")));
    }

    /// The shard for an org-addressed key is a function of the SCOPED key — the
    /// raw caller key hashes elsewhere. This is the drift the shared helper
    /// exists to prevent: an LB hashing the raw key would pick the wrong shard
    /// for nearly every request.
    #[test]
    fn org_scoping_changes_the_shard_mapping_and_isolates_orgs() {
        let n = 256;
        let key = "orders/checkout";
        let a = shard_for(&org_scoped_key("org_a", key), n);
        let b = shard_for(&org_scoped_key("org_b", key), n);
        // Deterministic per org…
        assert_eq!(a, shard_for(&org_scoped_key("org_a", key), n));
        assert_eq!(b, shard_for(&org_scoped_key("org_b", key), n));
        // …and a crafted key cannot collide into another org's scoped key,
        // because the delimiter cannot appear in a valid org id.
        assert_ne!(
            org_scoped_key("org_a", &format!("\u{1}org_b\u{1}{key}")),
            org_scoped_key("org_b", key),
        );
    }

    /// `nearest_to` returns the region that actually minimizes the haversine
    /// distance (guards the selection loop), and is deterministic.
    #[test]
    fn nearest_region_minimizes_distance() {
        for (lat, lon) in [
            (38.8977, -77.0365), // Washington, DC
            (41.25, -95.9),      // Omaha
            (50.1, 8.7),         // Frankfurt
            (35.68, 139.69),     // Tokyo (far from everything)
            (-33.87, 151.21),    // Sydney
            (0.0, 0.0),          // gulf of Guinea
        ] {
            let picked = Region::nearest_to(lat, lon);
            for other in Region::ALL {
                assert!(
                    picked.distance_km_to(lat, lon) <= other.distance_km_to(lat, lon),
                    "nearest_to({lat},{lon}) picked {} but {} is closer",
                    picked.code(),
                    other.code()
                );
            }
            assert_eq!(picked, Region::nearest_to(lat, lon));
        }
    }

    #[test]
    fn unknown_regional_routes_match_the_explicit_default_for_all_key_shapes() {
        let regions = ["gcp", "aws", "hetzner"];
        let long_key = "regional/".repeat(8_192);
        for shard_count in [3u32, 4, 16, 257] {
            for key in ["", "orders/结账", "a\0b", long_key.as_str()] {
                let unknown =
                    route_shard(KeyScope::Regional, key, "unknown", &regions, shard_count);
                let explicit = route_shard(KeyScope::Regional, key, "gcp", &regions, shard_count);
                assert_eq!(
                    unknown, explicit,
                    "unknown regions must stay in the configured primary band"
                );
            }
        }
    }
}
