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
