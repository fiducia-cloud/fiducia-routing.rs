//! Deterministic metamorphic conformance of region bands and per-org authority.
//! Exercises the published routing API rather than duplicating its algorithm.
use fiducia_routing::{
    org_scoped_key, route_shard, shard_for, shard_for_region, KeyScope,
};

#[test]
fn every_non_divisible_region_band_partitions_without_overlap() {
    for region_count in 1_u32..=9 {
        for shard_count in region_count..=96 {
            let band = shard_count / region_count;
            let mut occupied = vec![false; shard_count as usize];
            for index in 0..region_count {
                let begin = index * band;
                let end = if index == region_count - 1 {
                    shard_count
                } else {
                    begin + band
                };
                for key in ["", "a", "orders/🔒", "\u{1}org-a\u{1}key", "../"] {
                    let shard = shard_for_region(index, region_count, key, shard_count);
                    assert!(
                        (begin..end).contains(&shard),
                        "region {index}/{region_count} escaped [{begin}, {end}) at n={shard_count}"
                    );
                }
                for slot in begin..end {
                    assert!(!occupied[slot as usize], "regional shard bands overlap");
                    occupied[slot as usize] = true;
                }
            }
            assert!(occupied.iter().all(|seen| *seen), "regional partition leaves gaps");
        }
    }
}

#[test]
fn unknown_and_ambiguous_region_inputs_never_escape_default_band() {
    let regions = ["primary", "secondary", "tertiary"];
    for shard_count in 3_u32..=72 {
        for key in ["", "a", "🔒", "orders/42", "e\u{301}"] {
            let primary = route_shard(
                KeyScope::Regional,
                key,
                "primary",
                &regions,
                shard_count,
            );
            for input in ["", "no-match", " PRIMARY-extra ", "\0", "unknown:primary"] {
                assert_eq!(
                    route_shard(KeyScope::Regional, key, input, &regions, shard_count),
                    primary,
                    "untrusted/unknown region must deterministically use primary"
                );
            }
            assert_eq!(
                route_shard(
                    KeyScope::Regional,
                    key,
                    " SECONDARY ",
                    &regions,
                    shard_count,
                ),
                shard_for_region(1, 3, key, shard_count)
            );
        }
    }
}

#[test]
fn impossible_region_topology_fails_to_global_without_panicking() {
    let keys = ["", "key", "locks/🔒", "\0reserved"];
    for shard_count in 1_u32..=8 {
        for key in keys {
            for regions in [
                &[][..],
                &["a", "b", "c", "d", "e", "f", "g", "h", "i"][..],
            ] {
                if regions.is_empty() || regions.len() as u32 > shard_count {
                    assert_eq!(
                        route_shard(KeyScope::Regional, key, "spoofed", regions, shard_count),
                        shard_for(key, shard_count),
                    );
                }
            }
        }
    }
}

#[test]
fn tenant_scoped_global_keys_cannot_be_remapped_by_region_matrix() {
    let regions = ["gcp", "aws", "hetzner"];
    for tenant in ["tenant-a", "tenant-b", "\u{00e9}", "e\u{301}"] {
        for key in ["", "lock/42", "../", "🔑"] {
            let scoped = org_scoped_key(tenant, key);
            for shard_count in [1_u32, 3, 5, 16, 31, 64, 257] {
                let expected = shard_for(&scoped, shard_count);
                for region in ["gcp", "aws", "hetzner", "bad", "", " gCp "] {
                    assert_eq!(
                        route_shard(
                            KeyScope::Global,
                            &scoped,
                            region,
                            &regions,
                            shard_count,
                        ),
                        expected,
                        "region metadata must never be global-shard authority",
                    );
                }
            }
        }
    }
}
