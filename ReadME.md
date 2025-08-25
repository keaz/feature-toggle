
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

Lets implement authentication for the feature-toggle-backend. When service start first server should check whether there
is admin account exists or not. If the admin account is not configured, the request should be redirected to admin
creation page. This page is maintained in the React web application. Admin creation path is /createAdmin.
Introduce a mutation for create admin.
Design database for this. Table for this should be common for both admins and normal users. Use a column to
differentiate admin and normal users. There should be columns for username,password, created/updated date time, last
login time, should capture First name, last name, email. Password should be stored as salted hash.
We should be able to use a actix_web Middleware for check admin account exist or not.
this Middleware should run after the AcceslogMiddleware.
Use sqlx migration scripts to create user table. No need to specify database schema when creating table.
