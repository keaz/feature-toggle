
```shell
cargo install trunk
cargo install cargo-generate
cargo install leptosfmt --locked
cargo install cargo-leptos --locked
```

### For SSR 
```shell
cargo install wasm-pack
wasm-pack build --target=web --debug --no-default-features --features=hydrate
cargo run --no-default-features --features=ssr
```

grpcurl -plaintext -import-path ./proto -proto evaluation.proto \
-d '{"feature_id":"123e4567-e89b-12d3-a456-426614174000","environment_id":"prod","
context":[{"key":"user_id","value":"42"}]}' \
127.0.0.1:50051 FeatureEvaluation/Evaluate

grpcurl -plaintext -import-path ./proto -proto evaluation.proto \
-d '{"feature_id":"5eef17bc-9e06-411d-b5f4-7a786e68bb99","environment_id":"78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017","
context":[{"key":"user_id","value":"42"}]}' \
127.0.0.1:50051 featuretoggle.FeatureEvaluation/Evaluate