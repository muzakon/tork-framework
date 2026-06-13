//! End-to-end TLS tests (require the `tls` feature).
//!
//! A self-signed certificate is generated with rcgen, served by `App::tls`, and hit
//! by a real rustls client: one connection proves ALPN negotiates HTTP/2, another
//! proves an HTTP/1.1 request is served over TLS.
#![cfg(feature = "tls")]

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;
use tork::{get, App, TlsConfig};

#[get("/")]
async fn ping() -> tork::Result<&'static str> {
    Ok("pong")
}

/// A self-signed certificate for `localhost`: PEM for the server, DER for the
/// client's trust store.
struct TestCert {
    cert_pem: String,
    key_pem: String,
    cert_der: CertificateDer<'static>,
}

fn generate_cert() -> TestCert {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])
        .expect("generate self-signed certificate");
    TestCert {
        cert_pem: certified.cert.pem(),
        key_pem: certified.key_pair.serialize_pem(),
        cert_der: certified.cert.der().clone(),
    }
}

/// Starts the TLS server on an ephemeral port; returns the address and a shutdown
/// handle.
async fn serve_tls(cert: &TestCert) -> (std::net::SocketAddr, oneshot::Sender<()>) {
    let (addr_tx, addr_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(addr_tx)));

    let app = App::new()
        .include(ping)
        .tls(TlsConfig::from_pem(&cert.cert_pem, &cert.key_pem).expect("build tls config"))
        .on_ready(move |ctx| {
            let sender = sender.clone();
            async move {
                if let Some(tx) = sender.lock().unwrap().take() {
                    let _ = tx.send(ctx.addr());
                }
                Ok(())
            }
        });

    tokio::spawn(app.serve_with_shutdown("127.0.0.1:0", async move {
        let _ = shutdown_rx.await;
    }));

    let addr = addr_rx.await.expect("server bound");
    (addr, shutdown_tx)
}

/// Builds a rustls client connector that trusts `cert` and advertises `alpn`.
fn connector(cert: &TestCert, alpn: &[&[u8]]) -> TlsConnector {
    let mut roots = RootCertStore::empty();
    roots.add(cert.cert_der.clone()).expect("trust the test cert");

    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("client protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    TlsConnector::from(Arc::new(config))
}

#[tokio::test]
async fn tls_handshake_negotiates_http2_via_alpn() {
    let cert = generate_cert();
    let (addr, shutdown) = serve_tls(&cert).await;

    // Offer both protocols; the server prefers h2, so ALPN must resolve to h2.
    let connector = connector(&cert, &[b"h2", b"http/1.1"]);
    let name = ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let tls = connector.connect(name, tcp).await.expect("tls handshake");

    let (_, conn) = tls.get_ref();
    assert_eq!(
        conn.alpn_protocol(),
        Some(&b"h2"[..]),
        "TLS must negotiate HTTP/2 over ALPN"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn http1_request_is_served_over_tls() {
    let cert = generate_cert();
    let (addr, shutdown) = serve_tls(&cert).await;

    // An HTTP/1.1-only client sends a plain request over the encrypted connection.
    let connector = connector(&cert, &[b"http/1.1"]);
    let name = ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let mut tls = connector.connect(name, tcp).await.expect("tls handshake");

    tls.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    tls.read_to_end(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf);

    assert!(response.contains("200"), "expected a 200 status: {response}");
    assert!(response.contains("pong"), "expected the handler body: {response}");

    let _ = shutdown.send(());
}
