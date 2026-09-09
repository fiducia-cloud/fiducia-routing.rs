import "@typespec/json-schema";
using TypeSpec.JsonSchema;

namespace Fiducia.Routing;

@id("RoutingScope")
enum RoutingScope {
  global: "global",
  regional: "regional",
}

@id("HashRequest")
model HashRequest {
  op: "hash";
  key: string;
}

@id("ShardRequest")
model ShardRequest {
  op: "shard";
  key: string;
  shard_count: int32;
}

@id("ScopeRequest")
model ScopeRequest {
  op: "scope";
  org_id: string;
  key: string;
}

@id("RouteRequest")
model RouteRequest {
  op: "route";
  scope: RoutingScope;
  key: string;
  region?: string;
  regions?: string[];
  shard_count: int32;
}

@id("HashResult")
model HashResult {
  schema_version: int32;
  line: int32;
  op: "hash";
  result: int32;
}

@id("ShardResult")
model ShardResult {
  schema_version: int32;
  line: int32;
  op: "shard";
  result: int32;
}

@id("ScopeResult")
model ScopeResult {
  schema_version: int32;
  line: int32;
  op: "scope";
  result: string;
}

@id("RouteResult")
model RouteResult {
  schema_version: int32;
  line: int32;
  op: "route";
  result: int32;
}
