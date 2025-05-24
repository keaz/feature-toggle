
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