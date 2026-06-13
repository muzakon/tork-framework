# TLS and HTTP/2

Tork can terminate TLS itself and tune the HTTP/1 and HTTP/2 connection layers. TLS
lives behind the `tls` feature (off by default so the base build stays lean):

```toml
tork = { version = "0.1", features = ["tls"] }
```

It uses rustls with the ring provider — pure Rust, no OpenSSL and no C toolchain.

## Enabling TLS

Pass a `TlsConfig` to `App::tls`. A certificate and key are given as PEM **file
paths** or as PEM **contents**:

```rust
use tork::{App, TlsConfig};

App::new()
    .include(index)
    .tls(TlsConfig::from_pem_files("./cert.pem", "./key.pem")?)
    .serve("0.0.0.0:443")
    .await?;
```

`cert.pem` is a certificate chain (leaf first); `key.pem` is a PKCS#8, PKCS#1, or
SEC1 private key. A malformed certificate or key fails fast at boot, not on the first
connection. With TLS set, the startup log shows `https://`.

HTTP/2 is negotiated automatically: ALPN advertises `h2` then `http/1.1` by default,
so a modern client gets HTTP/2 and older clients fall back to HTTP/1.1. Call
`.http1_only()` on the config to advertise only HTTP/1.1.

## SNI: multiple certificates

Serve different certificates per host name (Server Name Indication). The certificate
from `from_pem*` is the default (served when no name matches); add more with
`add_sni_cert_pem*`:

```rust
let tls = TlsConfig::from_pem_files("./default-cert.pem", "./default-key.pem")?
    .add_sni_cert_pem_files("api.example.com", "./api-cert.pem", "./api-key.pem")?
    .add_sni_cert_pem_files("admin.example.com", "./admin-cert.pem", "./admin-key.pem")?;
```

## Mutual TLS (client certificates)

Require, or optionally request, a client certificate signed by a CA you trust:

```rust
let ca = std::fs::read("./client-ca.pem")?;
let tls = TlsConfig::from_pem_files("./cert.pem", "./key.pem")?
    .client_auth_required_pem(&ca)?;   // or .client_auth_optional_pem(&ca)?
```

`client_auth_required_pem` rejects any connection without a valid client
certificate; `client_auth_optional_pem` verifies one when present but still allows
clients that send none.

## HTTP/2 and HTTP/1 tuning

Tune the protocol layers for every connection. Unset fields keep hyper's defaults:

```rust
use std::time::Duration;
use tork::{App, Http1Config, Http2Config};

App::new()
    .http2(
        Http2Config::new()
            .max_concurrent_streams(256)               // streams per connection
            .keep_alive_interval(Duration::from_secs(20))
            .keep_alive_timeout(Duration::from_secs(10))
            .initial_stream_window_size(1 << 20)
            .initial_connection_window_size(1 << 21)
            .max_frame_size(1 << 14)
            .max_header_list_size(16 * 1024),
    )
    .http1(Http1Config::new().keep_alive(true).max_headers(100));
```

These apply whether or not TLS is enabled (HTTP/2 also works cleartext via the h2c
upgrade). The slowloris-bounding request-head read timeout is separate — see
`App::header_read_timeout`.

## Behind a reverse proxy

Terminating TLS at Tork is fully supported, but many deployments terminate TLS at a
reverse proxy or load balancer (nginx, Caddy, a cloud LB) and forward plain HTTP to
Tork on a private network. In that setup, leave TLS off here and configure
`ProxyHeaders` (with a trusted-proxy allowlist) and `TrustedHost` so forwarded
scheme/host are honored only from the proxy.

## HTTP/3

HTTP/3 (QUIC) is not yet supported — it needs a separate UDP/QUIC server stack. It is
on the roadmap; for now use HTTP/2 over TLS.
