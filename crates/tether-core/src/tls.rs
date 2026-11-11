//! TLS configuration. Both sides present self-signed certificates; we accept any certificate
//! during the handshake and decide trust afterwards by comparing with the certificate pinned at
//! pairing time (see `service`). Signatures are still verified, so the peer must own the key.

use std::sync::Arc;

use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider},
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme,
};

use crate::{cert::LocalCert, Result};

#[derive(Clone)]
pub struct TlsContext {
    pub server: Arc<ServerConfig>,
    pub client: Arc<ClientConfig>,
}

impl TlsContext {
    pub fn new(local: &LocalCert) -> Result<Self> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(AcceptAny(provider.clone()));
        let versions = &[&rustls::version::TLS13, &rustls::version::TLS12];
        let chain = || vec![CertificateDer::from(local.cert_der.clone())];
        let key = || PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(local.key_der.clone()));

        let server = ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(versions)?
            .with_client_cert_verifier(verifier.clone())
            .with_single_cert(chain(), key())?;
        let client = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_auth_cert(chain(), key())?;
        Ok(TlsContext {
            server: Arc::new(server),
            client: Arc::new(client),
        })
    }

    /// SNI is meaningless here; the peer ignores it.
    pub fn server_name() -> ServerName<'static> {
        ServerName::try_from("kdeconnect")
            .unwrap_or(ServerName::IpAddress(std::net::Ipv4Addr::LOCALHOST.into()))
    }
}

#[derive(Debug)]
struct AcceptAny(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
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
        verify_tls13_signature(
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

impl ClientCertVerifier for AcceptAny {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
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
        verify_tls13_signature(
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
