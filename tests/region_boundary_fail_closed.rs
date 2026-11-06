//! Boundary and negative-config checks for public region-sharding APIs.
//! The first region's placement must remain stable at the edge of u32 values.
use fiducia_routing::{
    lock_coordination_shard, region_index_or, route_shard, service_discovery_shard, shard_for,
    shard_for_region, KeyScope,
};

#[test]
fn excessive_region_index_clamps_to_last_band_without_wraparound() {
    for region_count in [1_u32, 2, 3, 7, 17] {
        for shard_count in [region_count, region_count + 1, region_count * 4 + 1, 257] {
            if shard_count < region_count {
                continue;
            }
            for key in ["", "alpha", "tenant/🔒", "\u{1}org-a\u{1}locks"] {
                let last = shard_for_region(region_count - 1, region_count, key, shard_count);
                for hostile_index in [region_count, region_count + 1, u32::MAX] {
                    let observed = shard_for_region(hostile_index, region_count, key, shard_count);
                    assert_eq!(
                        observed, last,
                        "out-of-range region index must be clamped to final region"
                    );
                }
            }
        }
    }
}

#[test]
fn missing_region_or_impossible_topology_cannot_split_global_lock_state() {
    for key in ["", "a/b", "\0internal", "🔒"] {
        for shard_count in [1_u32, 2, 3, 7, 16, 257] {
            let expected = shard_for(key, shard_count);
            assert_eq!(
                route_shard(KeyScope::Regional, key, "unknown", &[], shard_count),
                expected
            );
            if shard_count < 10 {
                let oversized = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
                assert_eq!(
                    route_shard(KeyScope::Regional, key, "a", &oversized, shard_count),
                    expected,
                    "undersized shard count must fall back to global routing"
                );
            }
            assert_eq!(
                lock_coordination_shard(shard_count),
                shard_for("\0fiducia-lock-coordinator", shard_count)
            );
            assert_eq!(
                service_discovery_shard(shard_count),
                shard_for("\0fiducia-service-discovery", shard_count)
            );
        }
    }
}

#[test]
fn zero_config_is_rejected_before_dividing_or_indexing_region_bands() {
    for (region_count, shard_count) in [(0_u32, 0_u32), (0, 1), (0, 17), (1, 0), (3, 2)] {
        let failure =
            std::panic::catch_unwind(|| shard_for_region(0, region_count, "a", shard_count));
        assert!(
            failure.is_err(),
            "invalid {region_count}/{shard_count} must fail"
        );
    }
    assert!(std::panic::catch_unwind(|| shard_for("a", 0)).is_err());
    assert!(std::panic::catch_unwind(|| lock_coordination_shard(0)).is_err());
    assert!(std::panic::catch_unwind(|| service_discovery_shard(0)).is_err());
}

#[test]
fn regional_header_defaults_do_not_change_explicitly_selected_global_route() {
    let known = ["gcp", "aws", "hetzner"];
    for supplied in ["", "bad", " GcP ", "aws", "HETZNER", "bad\nregion"] {
        let index = region_index_or(supplied, &known, 0);
        assert!(index < known.len() as u32);
        for count in [3_u32, 4, 5, 16, 256] {
            let chosen = route_shard(
                KeyScope::Global,
                "shared/coordination",
                supplied,
                &known,
                count,
            );
            assert_eq!(chosen, shard_for("shared/coordination", count));
        }
    }
}

// Wave50: preserve the public routing ABI under independent negative inputs.
#[test]
fn wave50_region_aliases_normalize_without_accepting_prefix_spoofs() {
    use fiducia_routing::Region;
    for (input, expected) in [
        (" GCP ", Region::UsCentral1),
        ("uS-CeNtRaL", Region::UsCentral1),
        ("us-east", Region::UsEast1),
        (" AWS ", Region::UsEast1),
        ("eu-central-1", Region::EuCentral),
        ("Hetzner", Region::EuCentral),
    ] {
        assert_eq!(Region::parse(input), Some(expected), "{input}");
    }
    for invalid in ["gcp-extra", "aws.example", "eu-central1", "gcp\naws", ""] {
        assert_eq!(Region::parse(invalid), None, "{invalid}");
    }
}

#[test]
fn wave50_utf8_framing_hashes_bytes_not_unicode_code_points() {
    for key in ["", "a\0b", "café", "☃️", "🚀/東京", "\u{1}tenant\u{1}🔒"] {
        let independent = key.as_bytes().iter().fold(0x811c_9dc5_u32, |hash, byte| {
            (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
        });
        assert_eq!(fiducia_routing::fnv1a(key), independent, "{key:?}");
        for count in [1, 2, 3, 17, 257, 1024] {
            assert_eq!(shard_for(key, count), independent % count);
        }
    }
}

#[test]
fn wave50_org_prefix_enforces_exact_tenant_boundary() {
    use fiducia_routing::{org_scope_prefix, org_scoped_key};
    for (tenant, neighboring) in [
        ("acme", "acme-2"),
        ("a", "ab"),
        ("日本語", "日本語2"),
        ("tenant/one", "tenant"),
    ] {
        let prefix = org_scope_prefix(tenant);
        let scoped = org_scoped_key(tenant, "📦/\0record");
        let neighbor = org_scoped_key(neighboring, "📦/\0record");
        assert!(scoped.starts_with(&prefix));
        assert!(!neighbor.starts_with(&prefix));
        assert_eq!(scoped.strip_prefix(&prefix), Some("📦/\0record"));
    }
}

#[test]
fn wave50_disjoint_regional_bands_remain_disjoint_under_uneven_counts() {
    for regions in [2_u32, 3, 5, 7] {
        for extra in [0_u32, 1, 2, 3, 11, 53] {
            let shards = regions + extra;
            let band = shards / regions;
            for ri in 0..regions {
                let start = ri * band;
                let end = if ri + 1 == regions {
                    shards
                } else {
                    start + band
                };
                for key in ["", "a", "Ω", "tenant/\0state", "📦"] {
                    let actual = shard_for_region(ri, regions, key, shards);
                    assert!(actual >= start && actual < end, "{regions}/{shards}/{ri}");
                }
            }
        }
    }
}

#[test]
fn wave50_region_header_normalization_preserves_exact_shard_choice() {
    let regions = ["gcp", "aws", "hetzner"];
    for (normalized, aliases) in [
        ("gcp", ["GCP", " gcp ", "GcP"]),
        ("aws", ["AWS", " aWs ", "aws"]),
        ("hetzner", ["HETZNER", " Hetzner ", "hetzner"]),
    ] {
        for key in ["", "tenant/∂", "locks/global", "\0coordinator"] {
            let target = route_shard(KeyScope::Regional, key, normalized, &regions, 103);
            for alias in aliases {
                assert_eq!(
                    route_shard(KeyScope::Regional, key, alias, &regions, 103),
                    target
                );
            }
        }
    }
}
