# HTTP Parity Benchmarks

This crate compares Tork and Axum over the same HTTP-core scenarios:

- `json_ok`
- `path_param`
- `json_validate`
- `middleware_stack`
- `typed_error`

Commands:

```sh
cargo test -p http-parity
cargo bench -p http-parity --bench http_core
cargo run -p http-parity --bin parity_load --release -- tork json_ok --concurrency 16 --duration 20
cargo run -p http-parity --bin parity_server --release -- axum middleware_stack 127.0.0.1:3000
```

The binaries use real TCP sockets. `parity_load` spawns and tears down the server
for you. `parity_server` is for external profilers and ad-hoc inspection.
