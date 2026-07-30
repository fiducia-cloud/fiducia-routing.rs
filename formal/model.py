#!/usr/bin/env python3
"""Bounded executable model for fiducia-routing's frozen key-to-shard contract."""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from pathlib import Path
from typing import Any, Iterable

MASK32 = 0xFFFF_FFFF
FNV_OFFSET = 0x811C_9DC5
FNV_PRIME = 0x0100_0193
ORG_SCOPE_DELIM = "\x01"
LOCK_COORDINATION_KEY = "\x00fiducia-lock-coordinator"
SERVICE_DISCOVERY_KEY = "\x00fiducia-service-discovery"
DEFAULT_REGION_INDEX = 0


def fnv1a(value: str) -> int:
    digest = FNV_OFFSET
    for byte in value.encode("utf-8"):
        digest ^= byte
        digest = (digest * FNV_PRIME) & MASK32
    return digest


def shard_for(key: str, shard_count: int) -> int:
    if shard_count <= 0:
        raise ValueError("shard_count must be > 0")
    return fnv1a(key) % shard_count


def org_scoped_key(org_id: str, key: str) -> str:
    return f"{ORG_SCOPE_DELIM}{org_id}{ORG_SCOPE_DELIM}{key}"


def region_index(region: str, regions: tuple[str, ...]) -> int | None:
    normalized = region.strip().casefold()
    for index, candidate in enumerate(regions):
        if candidate.casefold() == normalized:
            return index
    return None


def shard_for_region(region_index_value: int, region_count: int, key: str, shard_count: int) -> int:
    if region_count <= 0:
        raise ValueError("region_count must be > 0")
    if shard_count < region_count:
        raise ValueError("need at least one shard per region")
    resolved = min(region_index_value, region_count - 1)
    band = shard_count // region_count
    base = resolved * band
    size = shard_count - base if resolved == region_count - 1 else band
    return base + (fnv1a(key) % size)


def route_shard(scope: str, key: str, region: str, regions: tuple[str, ...], shard_count: int) -> int:
    if scope == "global":
        return shard_for(key, shard_count)
    if scope != "regional":
        raise ValueError("scope must be global or regional")
    if not regions or shard_count < len(regions):
        return shard_for(key, shard_count)
    resolved = region_index(region, regions)
    if resolved is None:
        resolved = DEFAULT_REGION_INDEX
    return shard_for_region(resolved, len(regions), key, shard_count)


def band_bounds(index: int, region_count: int, shard_count: int) -> tuple[int, int]:
    band = shard_count // region_count
    start = index * band
    end = shard_count if index == region_count - 1 else start + band
    return start, end


def load_manifest() -> dict[str, Any]:
    path = Path(__file__).with_name("fm.toml")
    with path.open("rb") as handle:
        manifest = tomllib.load(handle)
    assert manifest["schema_version"] == 1
    assert manifest["adapter_protocol"] == "json-stdin/v1"
    assert manifest["model"] == "formal/model.py"
    assert {entry["id"] for entry in manifest["invariants"]} == {
        "route-in-range", "global-region-independence", "regional-band-containment",
        "safe-global-fallback", "org-scope-injective", "coordinator-placement-frozen",
    }
    return manifest


def verify() -> dict[str, Any]:
    manifest = load_manifest()
    keys = ("", "a", "orders/42", "orders/checkout", "inventory/sku-9", "café", LOCK_COORDINATION_KEY, SERVICE_DISCOVERY_KEY)
    orgs = ("a", "b", "tenant-1", "tenant-2")
    regions = ("gcp", "aws", "hetzner")
    region_inputs = ("gcp", " AWS ", "HeTzNeR", "azure", "", "garbage")
    shard_counts = (1, 2, 3, 4, 5, 7, 12, 16, 64, 256, 1024)
    checks = 0

    for key in keys:
        for count in shard_counts:
            direct = shard_for(key, count)
            assert 0 <= direct < count
            assert direct == shard_for(key, count)
            checks += 2
            for region in region_inputs:
                routed = route_shard("global", key, region, regions, count)
                assert routed == direct
                assert 0 <= routed < count
                checks += 2
            assert route_shard("regional", key, "aws", (), count) == direct
            too_many_regions = tuple(f"r{i}" for i in range(count + 1))
            assert route_shard("regional", key, "r0", too_many_regions, count) == direct
            checks += 2
            if count >= len(regions):
                for region_value in region_inputs:
                    resolved = region_index(region_value, regions)
                    if resolved is None:
                        resolved = DEFAULT_REGION_INDEX
                    routed = route_shard("regional", key, region_value, regions, count)
                    start, end = band_bounds(resolved, len(regions), count)
                    assert start <= routed < end
                    checks += 1

    scoped: dict[str, tuple[str, str]] = {}
    for org in orgs:
        for key in keys:
            value = org_scoped_key(org, key)
            assert value.startswith(f"{ORG_SCOPE_DELIM}{org}{ORG_SCOPE_DELIM}")
            assert value not in scoped, (value, scoped[value], (org, key))
            scoped[value] = (org, key)
            checks += 2

    assert LOCK_COORDINATION_KEY.startswith("\x00")
    assert SERVICE_DISCOVERY_KEY.startswith("\x00")
    assert LOCK_COORDINATION_KEY != SERVICE_DISCOVERY_KEY
    for count, lock_shard, discovery_shard in ((16, 15, 9), (256, 223, 233), (1024, 223, 233)):
        assert shard_for(LOCK_COORDINATION_KEY, count) == lock_shard
        assert shard_for(SERVICE_DISCOVERY_KEY, count) == discovery_shard
        checks += 2

    return {"status": "ok", "model": manifest["id"], "claim": manifest["claim"], "checks": checks, "bounded_pairs": len(keys) * len(orgs)}


def emit(records: Iterable[dict[str, Any]]) -> None:
    for record in records:
        print(json.dumps(record, sort_keys=True, separators=(",", ":")))


def replay() -> None:
    load_manifest()
    outputs: list[dict[str, Any]] = []
    for line_number, raw in enumerate(sys.stdin, start=1):
        raw = raw.strip()
        if not raw:
            continue
        request = json.loads(raw)
        op = request.get("op")
        if op == "hash":
            result: Any = fnv1a(str(request["key"]))
        elif op == "shard":
            result = shard_for(str(request["key"]), int(request["shard_count"]))
        elif op == "scope":
            result = org_scoped_key(str(request["org_id"]), str(request["key"]))
        elif op == "route":
            result = route_shard(str(request["scope"]), str(request["key"]), str(request.get("region", "")), tuple(str(value) for value in request.get("regions", [])), int(request["shard_count"]))
        else:
            raise ValueError(f"line {line_number}: unsupported op {op!r}")
        outputs.append({"schema_version": 1, "line": line_number, "op": op, "result": result})
    emit(outputs)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--json-stdin", action="store_true")
    args = parser.parse_args()
    if args.json_stdin:
        replay()
    else:
        print(json.dumps(verify(), sort_keys=True))


if __name__ == "__main__":
    main()
