//! Versioned lock-coordination-domain routing.
//!
//! The legacy Fiducia lock path deliberately routes every lock and semaphore to
//! one reserved coordinator key. That keeps arbitrary multi-key union locks
//! atomic, but also makes unrelated tenants share one coordinator actor.
//!
//! This module introduces the routing contract needed to spread independent
//! coordination domains across Raft shards without silently remapping existing
//! clusters. It does **not** activate the new routing mode by itself: callers
//! must explicitly select [`LockRoutingMode::DomainV1`]. The default remains
//! [`LockRoutingMode::LegacyGlobal`].
//!
//! A coordination domain is intentionally coarser than an individual lock key.
//! Every key participating in one arbitrary union-lock namespace must resolve to
//! the same domain/state machine. Tenant-domain sharding therefore removes
//! cross-tenant contention, but it does not magically provide intra-tenant
//! parallelism for arbitrary union locks. That requires either explicit lock
//! spaces/subdomains or a future cross-shard atomic protocol.

use crate::{lock_coordination_shard, shard_for, ShardId};

/// Frozen prefix for the first version of domain-aware lock routing.
///
/// The leading NUL keeps this internal routing namespace separate from user
/// keys. Changing this string after DomainV1 is deployed is a data migration.
pub const LOCK_COORDINATION_DOMAIN_V1_PREFIX: &str = "\u{0}fiducia-lock-domain-v1";

/// The authority namespace whose lock/semaphore state must be serialized
/// together.
///
/// `Tenant` is the normal multi-tenant case. `Shared` is reserved for an
/// explicitly authorized cross-tenant collaboration space. `System` is for
/// Fiducia-owned coordination and must never be selectable directly by an
/// untrusted customer request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockCoordinationDomain<'a> {
    Tenant(&'a str),
    Shared(&'a str),
    System(&'a str),
}

impl<'a> LockCoordinationDomain<'a> {
    fn kind_and_id(self) -> (&'static str, &'a str) {
        match self {
            Self::Tenant(id) => ("tenant", id),
            Self::Shared(id) => ("shared", id),
            Self::System(id) => ("system", id),
        }
    }
}

/// Lock-routing protocol selection.
///
/// Existing clusters remain on `LegacyGlobal` until an explicit, coordinated
/// migration enables `DomainV1` in every component that predicts lock routing
/// (node, load balancer, brain, clients/operator tooling as applicable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LockRoutingMode {
    /// Existing behavior: every lock and semaphore routes through
    /// `LOCK_COORDINATION_KEY`.
    #[default]
    LegacyGlobal,
    /// Route by an explicit tenant/shared/system coordination domain.
    DomainV1,
}

/// Construct the stable internal routing key for one coordination domain.
///
/// The identifier is byte-length framed so variant/id boundaries remain
/// unambiguous even if an internal identifier contains separator characters.
/// Authentication and authorization happen above this pure routing helper; in
/// particular, callers must not turn an untrusted body field directly into a
/// `Tenant`, `Shared`, or `System` authority domain.
pub fn lock_coordination_domain_key(domain: LockCoordinationDomain<'_>) -> String {
    let (kind, id) = domain.kind_and_id();
    format!(
        "{LOCK_COORDINATION_DOMAIN_V1_PREFIX}\u{0}{kind}\u{0}{}\u{0}{id}",
        id.len()
    )
}

/// Resolve the Raft shard that owns a lock coordination domain under a selected
/// routing protocol.
///
/// In `LegacyGlobal` mode the domain is deliberately ignored, preserving the
/// existing persisted placement exactly. In `DomainV1`, all operations in one
/// domain converge on the same shard while independent domains can spread over
/// the cluster.
#[inline]
pub fn lock_coordination_shard_for(
    mode: LockRoutingMode,
    domain: LockCoordinationDomain<'_>,
    shard_count: u32,
) -> ShardId {
    match mode {
        LockRoutingMode::LegacyGlobal => lock_coordination_shard(shard_count),
        LockRoutingMode::DomainV1 => {
            let key = lock_coordination_domain_key(domain);
            shard_for(&key, shard_count)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fnv1a, LOCK_COORDINATION_KEY};

    #[test]
    fn legacy_is_the_default_and_preserves_existing_placement() {
        assert_eq!(LockRoutingMode::default(), LockRoutingMode::LegacyGlobal);

        for n in [1u32, 8, 16, 256, 1024] {
            let a = lock_coordination_shard_for(
                LockRoutingMode::default(),
                LockCoordinationDomain::Tenant("org-a"),
                n,
            );
            let b = lock_coordination_shard_for(
                LockRoutingMode::default(),
                LockCoordinationDomain::Tenant("org-b"),
                n,
            );
            assert_eq!(a, lock_coordination_shard(n));
            assert_eq!(b, lock_coordination_shard(n));
            assert_eq!(a, shard_for(LOCK_COORDINATION_KEY, n));
        }
    }

    #[test]
    fn tenant_domains_are_distinct_even_for_the_same_user_lock_name() {
        let a = lock_coordination_domain_key(LockCoordinationDomain::Tenant("org-a"));
        let b = lock_coordination_domain_key(LockCoordinationDomain::Tenant("org-b"));
        assert_ne!(a, b);
        assert!(a.starts_with(LOCK_COORDINATION_DOMAIN_V1_PREFIX));
        assert!(b.starts_with(LOCK_COORDINATION_DOMAIN_V1_PREFIX));
    }

    #[test]
    fn domain_variant_is_part_of_the_authority_identity() {
        let id = "workspace-7";
        assert_ne!(
            lock_coordination_domain_key(LockCoordinationDomain::Tenant(id)),
            lock_coordination_domain_key(LockCoordinationDomain::Shared(id))
        );
        assert_ne!(
            lock_coordination_domain_key(LockCoordinationDomain::Shared(id)),
            lock_coordination_domain_key(LockCoordinationDomain::System(id))
        );
    }

    #[test]
    fn every_union_lock_key_in_one_domain_co_locates_by_construction() {
        // The user/member keys are intentionally absent from the shard function:
        // arbitrary {a,b,c} sets stay on the one state machine for this domain.
        let domain = LockCoordinationDomain::Tenant("org-a");
        for n in [1u32, 8, 16, 256, 1024] {
            let expected = lock_coordination_shard_for(LockRoutingMode::DomainV1, domain, n);
            for _member_key in ["orders/42", "inventory/sku-9", "customer/7", "anything"] {
                assert_eq!(
                    lock_coordination_shard_for(LockRoutingMode::DomainV1, domain, n),
                    expected
                );
            }
        }
    }

    #[test]
    fn byte_length_framing_keeps_domain_keys_unambiguous() {
        let short = lock_coordination_domain_key(LockCoordinationDomain::Tenant("a\0bc"));
        let other = lock_coordination_domain_key(LockCoordinationDomain::Tenant("a\0b\0c"));
        assert_ne!(short, other);
        assert!(short.contains("\0tenant\0\x34\0"));
        assert!(other.contains("\0tenant\0\x35\0"));
    }

    /// Golden vectors pin DomainV1 before any production caller enables it.
    /// If these move after rollout, treat it as a lock-state migration rather
    /// than updating the expected values casually.
    #[test]
    fn domain_v1_golden_vectors_are_frozen() {
        let tenant_a = lock_coordination_domain_key(LockCoordinationDomain::Tenant("org_a"));
        let tenant_b = lock_coordination_domain_key(LockCoordinationDomain::Tenant("org_b"));
        let shared = lock_coordination_domain_key(LockCoordinationDomain::Shared("workspace-7"));
        let system = lock_coordination_domain_key(LockCoordinationDomain::System("cron"));

        assert_eq!(fnv1a(&tenant_a), 0xd02d_e878);
        assert_eq!(fnv1a(&tenant_b), 0xd32d_ed31);
        assert_eq!(fnv1a(&shared), 0x8d7b_5367);
        assert_eq!(fnv1a(&system), 0x6a11_3910);

        assert_eq!(shard_for(&tenant_a, 256), 120);
        assert_eq!(shard_for(&tenant_b, 256), 49);
        assert_eq!(shard_for(&shared, 256), 103);
        assert_eq!(shard_for(&system, 256), 16);
    }
}
