//! The local self-signed certificate and helpers for peer certificates.
//! KDE Connect trusts by pinning the peer certificate at pairing time; there is no CA.

use std::{fs, path::Path};

use sha2::{Digest, Sha256};
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::{Error, Result};

pub struct LocalCert {
    pub cert_der: Vec<u8>,
    /// PKCS#8 private key. Never logged.
    pub key_der: Vec<u8>,
}

impl LocalCert {
    /// Loads `cert.der`/`key.der` from `dir`, creating them on first run.
    pub fn load_or_create(dir: &Path, device_id: &str) -> Result<Self> {
        let cert_path = dir.join("cert.der");
        let key_path = dir.join("key.der");
        if let (Ok(cert_der), Ok(key_der)) = (fs::read(&cert_path), fs::read(&key_path)) {
            if common_name(&cert_der).as_deref() == Some(device_id) {
                return Ok(LocalCert { cert_der, key_der });
            }
        }
        let cert = Self::generate(device_id)?;
        fs::create_dir_all(dir)?;
        fs::write(&cert_path, &cert.cert_der)?;
        write_private(&key_path, &cert.key_der)?;
        Ok(cert)
    }

    /// kdeconnect-kde:core/kdeconnectconfig.cpp `generateCertificate`: CN = device id,
    /// O = "KDE", OU = "KDE Connect" (kdeconnect-android:SslHelper.kt `initialiseCertificate`).
    /// The desktop side checks CN against the announced device id; keep it stable once paired.
    pub fn generate(device_id: &str) -> Result<Self> {
        use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SerialNumber};
        let key = KeyPair::generate().map_err(|e| Error::Cert(e.to_string()))?;
        let mut params =
            CertificateParams::new(Vec::<String>::new()).map_err(|e| Error::Cert(e.to_string()))?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, device_id);
        dn.push(DnType::OrganizationName, "KDE");
        dn.push(DnType::OrganizationalUnitName, "KDE Connect");
        params.distinguished_name = dn;
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2034, 1, 1);
        params.serial_number = Some(SerialNumber::from_slice(
            &uuid::Uuid::new_v4().as_bytes()[..10],
        ));
        let cert = params
            .self_signed(&key)
            .map_err(|e| Error::Cert(e.to_string()))?;
        Ok(LocalCert {
            cert_der: cert.der().to_vec(),
            key_der: key.serialize_der(),
        })
    }

    /// The private key as PKCS#8 PEM for OpenSSH. The phone's file server accepts the key of our
    /// pinned certificate (kdeconnect-android SftpPlugin `SimplePublicKeyAuthenticator`;
    /// kdeconnect-kde plugins/sftp/mounter.cpp passes it as `IdentityFile`).
    pub fn key_pem(&self) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut b64 = String::new();
        for c in self.key_der.chunks(3) {
            let n = c
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
            for i in 0..4 {
                b64.push(if i <= c.len() {
                    T[(n >> (18 - 6 * i) & 63) as usize] as char
                } else {
                    '='
                });
            }
        }
        let mut pem = String::from("-----BEGIN PRIVATE KEY-----\n");
        for line in b64.as_bytes().chunks(64) {
            pem.push_str(&String::from_utf8_lossy(line));
            pem.push('\n');
        }
        pem.push_str("-----END PRIVATE KEY-----\n");
        pem
    }
}

#[cfg(unix)]
pub(crate) fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    Ok(fs::write(path, data)?)
}

pub fn common_name(der: &[u8]) -> Option<String> {
    let (_, cert) = X509Certificate::from_der(der).ok()?;
    let cn = cert
        .subject()
        .iter_common_name()
        .next()?
        .as_str()
        .ok()?
        .to_string();
    Some(cn)
}

fn public_key_der(der: &[u8]) -> Option<Vec<u8>> {
    let (_, cert) = X509Certificate::from_der(der).ok()?;
    Some(cert.public_key().raw.to_vec())
}

/// Short code both screens show during pairing so the user can confirm no one is in the middle.
/// kdeconnect-kde:core/device.cpp `verificationKey`: SHA-256 over the larger public key DER, then
/// the smaller, then the pairing timestamp as decimal text; first 8 hex chars, upper case.
pub fn verification_key(local_der: &[u8], remote_der: &[u8], timestamp: i64) -> Option<String> {
    let mut a = public_key_der(local_der)?;
    let mut b = public_key_der(remote_der)?;
    if a < b {
        std::mem::swap(&mut a, &mut b);
    }
    let mut h = Sha256::new();
    h.update(&a);
    h.update(&b);
    h.update(timestamp.to_string().as_bytes());
    Some(hex::encode_upper(h.finalize())[..8].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_cert_has_device_id_as_cn() {
        let id = crate::identity::new_device_id();
        let c = LocalCert::generate(&id).ok();
        assert_eq!(c.and_then(|c| common_name(&c.cert_der)), Some(id));
    }

    #[test]
    fn verification_key_is_symmetric() {
        let a = LocalCert::generate(&crate::identity::new_device_id()).ok();
        let b = LocalCert::generate(&crate::identity::new_device_id()).ok();
        let (Some(a), Some(b)) = (a, b) else {
            panic!("cert generation failed")
        };
        let k1 = verification_key(&a.cert_der, &b.cert_der, 1700000000);
        let k2 = verification_key(&b.cert_der, &a.cert_der, 1700000000);
        assert_eq!(k1, k2);
        assert_eq!(k1.map(|k| k.len()), Some(8));
    }
}
