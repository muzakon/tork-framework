//! TLS termination for the server, powered by rustls (ring provider).
//!
//! Build a [`TlsConfig`] from a certificate and key — given as PEM file paths or as
//! PEM contents — and hand it to [`App::tls`](crate::App::tls). The same config
//! covers SNI (multiple certificates keyed by server name), mutual TLS (client
//! certificate verification), and ALPN (which advertises HTTP/2 by default, so a
//! TLS connection negotiates h2 automatically).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert, WebPkiClientVerifier};
use rustls::sign::CertifiedKey;
use rustls::{RootCertStore, ServerConfig};
use tokio_rustls::TlsAcceptor;

use crate::error::{Error, Result};

/// The default ALPN protocols advertised: HTTP/2 preferred, HTTP/1.1 fallback.
const DEFAULT_ALPN: &[&[u8]] = &[b"h2", b"http/1.1"];

/// A parsed certificate chain plus its private key.
#[derive(Clone)]
struct CertEntry {
    chain: Vec<CertificateDer<'static>>,
    key: Arc<PrivateKeyDer<'static>>,
}

/// How the server treats client certificates (mutual TLS).
#[derive(Clone)]
enum ClientAuth {
    /// No client certificate is requested.
    None,
    /// A client certificate is requested but a missing one is still allowed.
    Optional(RootCertStore),
    /// A client certificate signed by a trusted CA is required.
    Required(RootCertStore),
}

/// TLS configuration for [`App::tls`](crate::App::tls).
///
/// Start from [`TlsConfig::from_pem_files`] or [`TlsConfig::from_pem`]; add more
/// certificates for SNI with [`add_sni_cert_pem`](TlsConfig::add_sni_cert_pem),
/// require client certificates with
/// [`client_auth_required_pem`](TlsConfig::client_auth_required_pem), and adjust the
/// advertised protocols with [`alpn`](TlsConfig::alpn).
#[derive(Clone)]
pub struct TlsConfig {
    /// The certificate served when no SNI name matches (the default).
    default: CertEntry,
    /// Per-server-name certificates, selected by the TLS SNI extension.
    sni: Vec<(String, CertEntry)>,
    client_auth: ClientAuth,
    alpn: Vec<Vec<u8>>,
}

impl TlsConfig {
    /// Builds a config from PEM-encoded certificate and key **file paths**.
    ///
    /// `cert` is a certificate chain (leaf first) and `key` is a PKCS#8, PKCS#1, or
    /// SEC1 private key.
    pub fn from_pem_files(cert: impl AsRef<Path>, key: impl AsRef<Path>) -> Result<Self> {
        let cert = read_file(cert.as_ref())?;
        let key = read_file(key.as_ref())?;
        Self::from_pem(cert, key)
    }

    /// Builds a config from PEM-encoded certificate and key **contents**.
    pub fn from_pem(cert: impl AsRef<[u8]>, key: impl AsRef<[u8]>) -> Result<Self> {
        let default = parse_cert_entry(cert.as_ref(), key.as_ref())?;
        Ok(Self {
            default,
            sni: Vec::new(),
            client_auth: ClientAuth::None,
            alpn: DEFAULT_ALPN.iter().map(|p| p.to_vec()).collect(),
        })
    }

    /// Adds an SNI certificate (PEM file paths) served for `server_name`.
    pub fn add_sni_cert_pem_files(
        self,
        server_name: impl Into<String>,
        cert: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> Result<Self> {
        let cert = read_file(cert.as_ref())?;
        let key = read_file(key.as_ref())?;
        self.add_sni_cert_pem(server_name, cert, key)
    }

    /// Adds an SNI certificate (PEM contents) served for `server_name`.
    pub fn add_sni_cert_pem(
        mut self,
        server_name: impl Into<String>,
        cert: impl AsRef<[u8]>,
        key: impl AsRef<[u8]>,
    ) -> Result<Self> {
        let entry = parse_cert_entry(cert.as_ref(), key.as_ref())?;
        self.sni.push((server_name.into(), entry));
        Ok(self)
    }

    /// Requires every client to present a certificate signed by a CA in `ca` (PEM).
    pub fn client_auth_required_pem(mut self, ca: impl AsRef<[u8]>) -> Result<Self> {
        self.client_auth = ClientAuth::Required(parse_roots(ca.as_ref())?);
        Ok(self)
    }

    /// Requests a client certificate, verifying it against `ca` (PEM) when present
    /// but still allowing clients that send none.
    pub fn client_auth_optional_pem(mut self, ca: impl AsRef<[u8]>) -> Result<Self> {
        self.client_auth = ClientAuth::Optional(parse_roots(ca.as_ref())?);
        Ok(self)
    }

    /// Overrides the advertised ALPN protocols (most-preferred first).
    ///
    /// The default is `["h2", "http/1.1"]`, which lets clients negotiate HTTP/2.
    pub fn alpn(mut self, protocols: Vec<Vec<u8>>) -> Self {
        self.alpn = protocols;
        self
    }

    /// Advertises only HTTP/1.1 (no HTTP/2) over TLS.
    pub fn http1_only(mut self) -> Self {
        self.alpn = vec![b"http/1.1".to_vec()];
        self
    }

    /// Builds the rustls acceptor, failing if any certificate/key is unusable.
    pub(crate) fn into_acceptor(self) -> Result<TlsAcceptor> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());

        let builder = ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|error| tls_error("could not select TLS protocol versions", error))?;

        let builder = match self.client_auth {
            ClientAuth::None => builder.with_no_client_auth(),
            ClientAuth::Optional(roots) => {
                let verifier =
                    WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                        .allow_unauthenticated()
                        .build()
                        .map_err(|error| tls_error("invalid client-auth CA", error))?;
                builder.with_client_cert_verifier(verifier)
            }
            ClientAuth::Required(roots) => {
                let verifier =
                    WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                        .build()
                        .map_err(|error| tls_error("invalid client-auth CA", error))?;
                builder.with_client_cert_verifier(verifier)
            }
        };

        let mut config = if self.sni.is_empty() {
            let key = clone_key(&self.default.key);
            builder
                .with_single_cert(self.default.chain.clone(), key)
                .map_err(|error| tls_error("invalid certificate or key", error))?
        } else {
            let resolver = SniResolver::build(&provider, &self.default, &self.sni)?;
            builder.with_cert_resolver(Arc::new(resolver))
        };

        config.alpn_protocols = self.alpn;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

/// Resolves a server certificate by the client's SNI name, with a default fallback.
#[derive(Debug)]
struct SniResolver {
    by_name: HashMap<String, Arc<CertifiedKey>>,
    default: Arc<CertifiedKey>,
}

impl SniResolver {
    fn build(
        provider: &Arc<rustls::crypto::CryptoProvider>,
        default: &CertEntry,
        sni: &[(String, CertEntry)],
    ) -> Result<Self> {
        let default = Arc::new(certified_key(provider, default)?);
        let mut by_name = HashMap::with_capacity(sni.len());
        for (name, entry) in sni {
            by_name.insert(name.clone(), Arc::new(certified_key(provider, entry)?));
        }
        Ok(Self { by_name, default })
    }
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        client_hello
            .server_name()
            .and_then(|name| self.by_name.get(name))
            .cloned()
            .or_else(|| Some(self.default.clone()))
    }
}

/// Turns a parsed cert entry into a rustls [`CertifiedKey`] using `provider`.
fn certified_key(
    provider: &Arc<rustls::crypto::CryptoProvider>,
    entry: &CertEntry,
) -> Result<CertifiedKey> {
    let signing_key = provider
        .key_provider
        .load_private_key(clone_key(&entry.key))
        .map_err(|error| tls_error("invalid private key", error))?;
    Ok(CertifiedKey::new(entry.chain.clone(), signing_key))
}

/// Parses a PEM certificate chain + private key into a [`CertEntry`].
fn parse_cert_entry(cert_pem: &[u8], key_pem: &[u8]) -> Result<CertEntry> {
    let chain = rustls_pemfile::certs(&mut &cert_pem[..])
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| tls_error("could not read certificate PEM", error))?;
    if chain.is_empty() {
        return Err(tls_message("certificate PEM contained no certificates"));
    }
    let key = rustls_pemfile::private_key(&mut &key_pem[..])
        .map_err(|error| tls_error("could not read private-key PEM", error))?
        .ok_or_else(|| tls_message("private-key PEM contained no key"))?;
    Ok(CertEntry {
        chain,
        key: Arc::new(key),
    })
}

/// Parses a PEM bundle of CA certificates into a root store.
fn parse_roots(ca_pem: &[u8]) -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &ca_pem[..]) {
        let cert = cert.map_err(|error| tls_error("could not read CA PEM", error))?;
        roots
            .add(cert)
            .map_err(|error| tls_error("invalid CA certificate", error))?;
    }
    if roots.is_empty() {
        return Err(tls_message("CA PEM contained no certificates"));
    }
    Ok(roots)
}

/// Clones a `PrivateKeyDer` (it is not `Clone`, so go through its bytes).
fn clone_key(key: &PrivateKeyDer<'static>) -> PrivateKeyDer<'static> {
    key.clone_key()
}

/// Reads a file into bytes, mapping IO errors to a config error.
fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|error| {
        tls_error(format!("could not read {}", path.display()), error)
    })
}

/// Builds a TLS config error from a message and a source error.
fn tls_error(message: impl Into<String>, source: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::internal(message).with_code("TLS_CONFIG").with_source(source)
}

/// Builds a TLS config error from a message alone.
fn tls_message(message: impl Into<String>) -> Error {
    Error::internal(message).with_code("TLS_CONFIG")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generates a self-signed certificate + key as PEM strings for `name`.
    fn self_signed(name: &str) -> (String, String) {
        let certified = rcgen::generate_simple_self_signed(vec![name.to_owned()])
            .expect("generate self-signed certificate");
        (certified.cert.pem(), certified.key_pair.serialize_pem())
    }

    #[test]
    fn from_pem_parses_and_builds_an_acceptor() {
        let (cert, key) = self_signed("localhost");
        let config = TlsConfig::from_pem(&cert, &key).expect("parse pem");
        assert_eq!(config.alpn, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
        config.into_acceptor().expect("build acceptor");
    }

    #[test]
    fn http1_only_drops_h2_from_alpn() {
        let (cert, key) = self_signed("localhost");
        let config = TlsConfig::from_pem(&cert, &key).unwrap().http1_only();
        assert_eq!(config.alpn, vec![b"http/1.1".to_vec()]);
    }

    #[test]
    fn sni_and_client_auth_variants_build() {
        let (cert, key) = self_signed("localhost");
        let (other_cert, other_key) = self_signed("example.com");
        let (ca, _) = self_signed("ca.example.com");
        let config = TlsConfig::from_pem(&cert, &key)
            .unwrap()
            .add_sni_cert_pem("example.com", &other_cert, &other_key)
            .unwrap()
            .client_auth_optional_pem(&ca)
            .unwrap();
        assert_eq!(config.sni.len(), 1);
        config.into_acceptor().expect("build acceptor with sni + mtls");
    }

    #[test]
    fn empty_certificate_pem_is_rejected() {
        let (_, key) = self_signed("localhost");
        match TlsConfig::from_pem("not a pem", &key) {
            Ok(_) => panic!("expected a TLS config error"),
            Err(error) => assert_eq!(error.code(), "TLS_CONFIG"),
        }
    }
}
