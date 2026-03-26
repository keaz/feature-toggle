# Contract-Driven SDK Strategy

This repository treats backend API contracts as generated artifacts. SDKs are generated from:

- OpenAPI: REST clients
- Protobuf (`evaluation.proto` + descriptor set): gRPC clients/stubs

## 1) Export contracts

From repository root:

```bash
./scripts/export-contracts.sh
```

The script defaults to `SQLX_OFFLINE=true` so it can run without live DB connectivity.

Artifacts are written to `feature-toggle-backend/contracts/generated/`:

- `openapi.json`
- `proto/evaluation.proto`
- `proto/evaluation_descriptor.pb`
- `contract-hashes.json`

Use a custom output directory:

```bash
./scripts/export-contracts.sh /tmp/feature-toggle-contracts
```

## 2) Compatibility guard (breaking-change check)

Run:

```bash
./scripts/check-contract-compat.sh
```

This runs the `feature-toggle-backend` contract compatibility test, which:

1. Re-exports OpenAPI + protobuf artifacts to a temp directory.
2. Recomputes SHA-256 hashes.
3. Compares against the versioned baseline:
   `feature-toggle-backend/contracts/baseline/contract-hashes.json`.

If hashes drift, the test fails (CI-safe behavior).

If a contract change is intentional:

```bash
./scripts/export-contracts.sh
cp feature-toggle-backend/contracts/generated/contract-hashes.json \
  feature-toggle-backend/contracts/baseline/contract-hashes.json
```

## 3) Generate TypeScript SDK (REST/OpenAPI)

Pinned OpenAPI Generator via Docker:

```bash
docker run --rm -v "$PWD:/local" openapitools/openapi-generator-cli:v7.12.0 generate \
  -i /local/feature-toggle-backend/contracts/generated/openapi.json \
  -g typescript-fetch \
  -o /local/sdk/ts/rest \
  --additional-properties=npmName=@fluxgate/sdk,typescriptThreePlus=true,supportsES6=true
```

## 4) Generate Java SDK (REST/OpenAPI)

```bash
docker run --rm -v "$PWD:/local" openapitools/openapi-generator-cli:v7.12.0 generate \
  -i /local/feature-toggle-backend/contracts/generated/openapi.json \
  -g java \
  -o /local/sdk/java/rest \
  --additional-properties=groupId=com.fluxgate,artifactId=fluxgate-sdk,artifactVersion=0.1.0,hideGenerationTimestamp=true
```

## 5) Generate Java gRPC stubs (protobuf)

Requires `protoc` and `protoc-gen-grpc-java` on `PATH`.

```bash
protoc \
  --proto_path=feature-toggle-backend/contracts/generated/proto \
  --java_out=sdk/java/grpc/src/main/java \
  --grpc-java_out=sdk/java/grpc/src/main/java \
  feature-toggle-backend/contracts/generated/proto/evaluation.proto
```

## 6) Generate TypeScript gRPC types (protobuf)

One option is `ts-proto`:

```bash
pnpm add -D ts-proto
protoc \
  --plugin=./node_modules/.bin/protoc-gen-ts_proto \
  --proto_path=feature-toggle-backend/contracts/generated/proto \
  --ts_proto_out=sdk/ts/grpc \
  --ts_proto_opt=esModuleInterop=true,outputServices=grpc-js \
  feature-toggle-backend/contracts/generated/proto/evaluation.proto
```

## 7) CI-friendly command pair

```bash
./scripts/export-contracts.sh
./scripts/check-contract-compat.sh
```
