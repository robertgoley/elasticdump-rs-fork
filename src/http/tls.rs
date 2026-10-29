//! rustls client configuration: custom CA bundles, `--insecure`, and client
//! certificate identities (including passphrase-protected PKCS#8 keys).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};

/// TLS material for one endpoint (input or output).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsOptions {
    /// PEM CA bundle; replaces the system roots (Node `ca` semantics).
    pub ca: Option<PathBuf>,
    /// PEM client certificate chain.
    pub cert: Option<PathBuf>,
    /// PEM private key (PKCS#8, PKCS#1/RSA, SEC1, or encrypted PKCS#8).
    pub key: Option<PathBuf>,
    /// Passphrase for an encrypted PKCS#8 key.
    pub pass: Option<String>,
    /// Skip server certificate verification entirely.
    pub insecure: bool,
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn build_client_config(opts: &TlsOptions) -> Result<ClientConfig> {
    let builder = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .context("Failed to select TLS protocol versions")?;

    let builder = if opts.insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerification(provider())))
    } else {
        builder.with_root_certificates(root_store(opts.ca.as_deref())?)
    };

    let config = match (&opts.cert, &opts.key) {
        (Some(cert), Some(key)) => {
            let chain = load_certs(cert)?;
            let key = load_key(key, opts.pass.as_deref())?;
            builder
                .with_client_auth_cert(chain, key)
                .context("Invalid client certificate/key pair")?
        }
        (Some(_), None) => bail!("A client certificate (--cert) requires a private key (--key)"),
        (None, Some(_)) => bail!("A private key (--key) requires a client certificate (--cert)"),
        (None, None) => builder.with_no_client_auth(),
    };
    Ok(config)
}

fn root_store(ca: Option<&Path>) -> Result<RootCertStore> {
    let mut store = RootCertStore::empty();
    match ca {
        Some(path) => {
            let certs = load_certs(path)?;
            let (added, _ignored) = store.add_parsable_certificates(certs);
            if added == 0 {
                bail!("No usable CA certificates found in '{}'", path.display());
            }
        }
        None => {
            let native = rustls_native_certs::load_native_certs();
            for err in &native.errors {
                log::debug!("Skipping unreadable system certificate: {err}");
            }
            let (added, _) = store.add_parsable_certificates(native.certs);
            if added == 0 {
                log::debug!("No system CA certificates found; using bundled webpki roots");
                store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            }
        }
    }
    Ok(store)
}

fn read(path: &Path, what: &str) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("Failed to read {what} '{}'", path.display()))
}

pub fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let pem = read(path, "certificate file")?;
    let certs = rustls_pemfile::certs(&mut pem.as_slice())
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("Failed to parse PEM certificates in '{}'", path.display()))?;
    if certs.is_empty() {
        bail!("No PEM certificates found in '{}'", path.display());
    }
    Ok(certs)
}

pub fn load_key(path: &Path, pass: Option<&str>) -> Result<PrivateKeyDer<'static>> {
    let pem = read(path, "private key file")?;
    let text = String::from_utf8_lossy(&pem);
    if text.contains("BEGIN ENCRYPTED PRIVATE KEY") {
        let pass = pass.ok_or_else(|| {
            anyhow!(
                "Private key '{}' is encrypted; supply its passphrase with --pass",
                path.display()
            )
        })?;
        let (label, doc) = pkcs8::SecretDocument::from_pem(&text)
            .map_err(|e| anyhow!("Failed to decode encrypted key '{}': {e}", path.display()))?;
        if label != "ENCRYPTED PRIVATE KEY" {
            bail!("Unexpected PEM label '{label}' in '{}'", path.display());
        }
        let info = pkcs8::EncryptedPrivateKeyInfo::try_from(doc.as_bytes())
            .map_err(|e| anyhow!("Invalid encrypted PKCS#8 key '{}': {e}", path.display()))?;
        let decrypted = info.decrypt(pass).map_err(|_| {
            anyhow!(
                "Failed to decrypt private key '{}' (wrong --pass?)",
                path.display()
            )
        })?;
        return Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            decrypted.as_bytes().to_vec(),
        )));
    }
    if text.contains("Proc-Type: 4,ENCRYPTED") {
        bail!(
            "Legacy encrypted PEM key '{}' is not supported; convert it with `openssl pkcs8 -topk8 -in {} -out key-pkcs8.pem`",
            path.display(),
            path.display()
        );
    }
    rustls_pemfile::private_key(&mut pem.as_slice())
        .with_context(|| format!("Failed to parse private key '{}'", path.display()))?
        .ok_or_else(|| anyhow!("No private key found in '{}'", path.display()))
}

/// Accept-anything verifier for `--insecure`. Signatures are still checked so
/// the handshake itself stays well-formed; only the chain/name are trusted.
#[derive(Debug)]
struct NoVerification(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for NoVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_builds_with_system_or_bundled_roots() {
        build_client_config(&TlsOptions::default()).unwrap();
    }

    #[test]
    fn insecure_config_builds() {
        build_client_config(&TlsOptions {
            insecure: true,
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn missing_ca_file_is_a_clear_error() {
        let err = build_client_config(&TlsOptions {
            ca: Some("/definitely/missing.pem".into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("missing.pem"));
    }

    #[test]
    fn cert_without_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.pem");
        std::fs::write(&p, "x").unwrap();
        let err = build_client_config(&TlsOptions {
            cert: Some(p),
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("--key"));
    }
}
