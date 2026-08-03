use fiducia_routing::{
    fnv1a, org_scope_prefix, org_scoped_key, route_shard, shard_for, KeyScope,
    ORG_SCOPE_DELIM,
};

#[test]
fn utf8_hash_vectors_are_frozen_for_every_sdk_implementation() {
    // FNV-1a consumes the exact UTF-8 bytes. These vectors cover multi-byte
    // scripts and canonically-equivalent strings so JS/Dart/Go/Rust clients do
    // not accidentally hash Unicode scalar values or normalize before routing.
    assert_eq!(fnv1a("🔒"), 0xad98_c8ba);
    assert_eq!(fnv1a("orders/结账"), 0xd80f_77e6);
    assert_eq!(fnv1a("é"), 0x1e9d_e8c1);
    assert_eq!(fnv1a("e\u{301}"), 0xfa9b_c71f);
    assert_ne!(
        fnv1a("é"),
        fnv1a("e\u{301}"),
        "routing is byte-preserving and must not perform hidden normalization"
    );
}

#[test]
fn org_scope_bytes_and_hash_are_a_persistent_cross_component_contract() {
    let scoped = org_scoped_key("org-a", "orders/42");

    assert_eq!(ORG_SCOPE_DELIM, '\u{1}');
    assert_eq!(scoped, "\u{1}org-a\u{1}orders/42");
    assert_eq!(org_scope_prefix("org-a"), "\u{1}org-a\u{1}");
    assert_eq!(fnv1a(&scoped), 0x8f26_77fd);

    for shard_count in [1u32, 8, 16, 257, 1024] {
        assert_eq!(
            shard_for(&scoped, shard_count),
            0x8f26_77fd % shard_count,
            "SDK and server must hash the scoped bytes before taking modulo"
        );
    }
}

#[test]
fn global_routes_ignore_all_region_metadata_shapes() {
    let key = "\u{1}org-a\u{1}locks/customer-42";
    let shard_count = 64;
    let expected = shard_for(key, shard_count);

    let region_sets: &[&[&str]] = &[
        &[],
        &["gcp"],
        &["gcp", "aws", "hetzner"],
        &["gcp", "gcp", "gcp"],
        &["bad.region", "*", ">", ""],
    ];

    for regions in region_sets {
        for supplied_region in ["gcp", "aws", "unknown", "", "  HETZNER  "] {
            assert_eq!(
                route_shard(
                    KeyScope::Global,
                    key,
                    supplied_region,
                    regions,
                    shard_count,
                ),
                expected,
                "global authority must never split because of client region metadata"
            );
        }
    }
}

#[test]
fn uneven_regional_bands_are_disjoint_and_unknown_defaults_to_primary() {
    let regions = ["gcp", "aws", "hetzner"];
    let shard_count = 10u32; // [0,3), [3,6), [6,10); last absorbs remainder.

    for key in ["orders/1", "orders/结账", "sessions/user-42", "🔒"] {
        let central = route_shard(
            KeyScope::Regional,
            key,
            "gcp",
            &regions,
            shard_count,
        );
        let east = route_shard(
            KeyScope::Regional,
            key,
            "aws",
            &regions,
            shard_count,
        );
        let europe = route_shard(
            KeyScope::Regional,
            key,
            "hetzner",
            &regions,
            shard_count,
        );

        assert!((0..3).contains(&central));
        assert!((3..6).contains(&east));
        assert!((6..10).contains(&europe));
        assert_ne!(central, east);
        assert_ne!(east, europe);
        assert_ne!(central, europe);

        assert_eq!(
            route_shard(
                KeyScope::Regional,
                key,
                "unknown-region",
                &regions,
                shard_count,
            ),
            central,
            "unknown region input must deterministically use the primary band"
        );
    }
}
