//! Cross-component lock-domain routing invariants over adversarial identifier
//! bytes. This is intentionally independent of the primary module's unit tests.
use fiducia_routing::{
    fnv1a, lock_coordination_shard, org_scoped_key, route_shard, shard_for,
    lock_coordination::{
        lock_coordination_domain_key, lock_coordination_shard_for,
        LockCoordinationDomain, LockRoutingMode, LOCK_COORDINATION_DOMAIN_V1_PREFIX,
    },
    KeyScope,
};

#[test]
fn framed_authority_keys_preserve_utf8_byte_length_and_domain_kind() {
    let ids = ["", "a", "a\0b", "a/b", "é", "e\u{301}", "🔒", "tenant\nid"];
    for id in ids {
        for (kind, domain) in [
            ("tenant", LockCoordinationDomain::Tenant(id)),
            ("shared", LockCoordinationDomain::Shared(id)),
            ("system", LockCoordinationDomain::System(id)),
        ] {
            let expected = format!(
                "{LOCK_COORDINATION_DOMAIN_V1_PREFIX}\0{kind}\0{}\0{id}",
                id.len()
            );
            let actual = lock_coordination_domain_key(domain);
            assert_eq!(actual.as_bytes(), expected.as_bytes(), "{kind}/{id:?}");
            assert!(actual.starts_with('\0'));
        }
        let tenant = lock_coordination_domain_key(LockCoordinationDomain::Tenant(id));
        assert_ne!(tenant, lock_coordination_domain_key(LockCoordinationDomain::Shared(id)));
        assert_ne!(tenant, lock_coordination_domain_key(LockCoordinationDomain::System(id)));
    }
    assert_ne!(
        lock_coordination_domain_key(LockCoordinationDomain::Tenant("é")),
        lock_coordination_domain_key(LockCoordinationDomain::Tenant("e\u{301}")),
        "normalizing a domain identifier would silently reassign persisted locks"
    );
}

#[test]
fn legacy_mode_cannot_be_selected_into_an_untrusted_tenant_shard() {
    for shard_count in [1_u32, 2, 3, 8, 16, 31, 256, 1024] {
        let reserved = lock_coordination_shard(shard_count);
        for id in ["", "org-a", "org-b", "a\0b", "/../", "🔒", "untrusted"] {
            for domain in [
                LockCoordinationDomain::Tenant(id),
                LockCoordinationDomain::Shared(id),
                LockCoordinationDomain::System(id),
            ] {
                assert_eq!(
                    lock_coordination_shard_for(
                        LockRoutingMode::LegacyGlobal,
                        domain,
                        shard_count
                    ),
                    reserved,
                    "legacy locks must never move with supplied domain labels"
                );
            }
        }
    }
}

#[test]
fn domain_v1_placement_matches_independent_canonical_key_hash() {
    for shard_count in [1_u32, 2, 7, 16, 257, 1024] {
        for id in ["", "org-a", "org-b", "a\0b", "a/b", "é", "e\u{301}", "🔒"] {
            for domain in [
                LockCoordinationDomain::Tenant(id),
                LockCoordinationDomain::Shared(id),
                LockCoordinationDomain::System(id),
            ] {
                let framed = lock_coordination_domain_key(domain);
                assert_eq!(
                    lock_coordination_shard_for(
                        LockRoutingMode::DomainV1,
                        domain,
                        shard_count
                    ),
                    fnv1a(&framed) % shard_count
                );
            }
        }
    }
}

#[test]
fn global_tenant_scoped_commands_are_immune_to_untrusted_region_metadata() {
    let regions: &[&[&str]] = &[
        &[],
        &["gcp", "aws", "hetzner"],
        &["west", "east", "invalid"],
        &["same", "same", "same"],
    ];
    for org in ["a", "b", "e\u{301}", "🔒"] {
        for key in ["", "locks/42", "a\0b", "../../relative", "🔑"] {
            let scoped = org_scoped_key(org, key);
            let expected = shard_for(&scoped, 257);
            for set in regions {
                for claimed_region in ["", "gcp", "aws", "invalid", " ../ ", "HETZNER"] {
                    assert_eq!(
                        route_shard(KeyScope::Global, &scoped, claimed_region, set, 257),
                        expected,
                        "global command crossed coordination authority"
                    );
                }
            }
        }
    }
}
