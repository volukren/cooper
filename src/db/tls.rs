use std::{
    fs,
    io::Write,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::Arc,
};

use p12_keystore::{Certificate, KeyStore, KeyStoreEntry, PrivateKey, PrivateKeyChain};

use super::{Failure, error_chain};
use crate::connection::{Connection, SslMode, expand_home};

const RSA_ENCRYPTION: &[u8] = &[
    0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01,
];
const EC_PUBLIC_KEY: &[u8] = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const DER_NULL: &[u8] = &[0x05, 0x00];
const SEQUENCE: u8 = 0x30;
const INTEGER: u8 = 0x02;
const OCTET_STRING: u8 = 0x04;
const OBJECT_IDENTIFIER: u8 = 0x06;
const EC_PARAMETERS: u8 = 0xa0;

struct Identity {
    pkcs12: Vec<u8>,
    password: String,
    native: native_tls::Identity,
}

struct Ca {
    ders: Vec<Vec<u8>>,
    certs: Vec<native_tls::Certificate>,
}

pub(super) struct Tls {
    mode: SslMode,
    ca: Option<Ca>,
    identity: Option<Identity>,
}

#[derive(Default)]
pub(super) struct TlsFiles {
    ca: Option<tempfile::TempPath>,
    identity: Option<tempfile::TempPath>,
}

impl Tls {
    pub fn load(conn: &Connection) -> Result<Self, String> {
        let mode = conn.ssl_mode;
        if !mode.uses_tls() {
            return Ok(Self {
                mode,
                ca: None,
                identity: None,
            });
        }
        let ca = match conn.ssl_ca.trim() {
            "" => None,
            path => {
                let (path, data) = read("CA cert", path)?;
                let ca = ca_certificates(&data)
                    .map_err(|reason| format!("CA cert {}: {reason}", path.display()))?;
                Some(ca)
            }
        };
        let identity = match (conn.ssl_cert.trim(), conn.ssl_key.trim()) {
            ("", "") => None,
            ("", _) => return Err("an SSL key needs an SSL cert as well".into()),
            (_, "") => return Err("an SSL cert needs an SSL key as well".into()),
            (cert, key) => {
                let (cert_path, cert) = read("SSL cert", cert)?;
                let (key_path, key) = read("SSL key", key)?;
                let certs = certificates(&cert)
                    .map_err(|reason| format!("SSL cert {}: {reason}", cert_path.display()))?;
                let key = private_key(&key)
                    .map_err(|reason| format!("SSL key {}: {reason}", key_path.display()))?;
                let password = random_password()?;
                let pkcs12 = pkcs12(key, certs, &password)?;
                let native = native_identity(&pkcs12, &password).map_err(|reason| {
                    format!(
                        "SSL cert {} and SSL key {} cannot be used together: {reason} \
                         (the key must belong to the first certificate in the cert file)",
                        cert_path.display(),
                        key_path.display()
                    )
                })?;
                Some(Identity {
                    pkcs12,
                    password,
                    native,
                })
            }
        };
        Ok(Self { mode, ca, identity })
    }

    pub fn verifies_certificate(&self) -> bool {
        matches!(self.mode, SslMode::VerifyCa | SslMode::VerifyFull)
    }

    pub fn verifies_host(&self) -> bool {
        self.mode == SslMode::VerifyFull
    }

    pub fn falls_back_to_plain_text(&self) -> bool {
        self.mode == SslMode::Prefer
    }

    pub fn postgres_mode(&self) -> tokio_postgres::config::SslMode {
        match self.mode {
            SslMode::Disable => tokio_postgres::config::SslMode::Disable,
            SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
            SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => {
                tokio_postgres::config::SslMode::Require
            }
        }
    }

    pub fn connector(&self) -> Result<native_tls::TlsConnector, String> {
        let mut builder = native_tls::TlsConnector::builder();
        builder
            .danger_accept_invalid_certs(!self.verifies_certificate())
            .danger_accept_invalid_hostnames(!self.verifies_host());
        for cert in self.ca.iter().flat_map(|ca| &ca.certs) {
            builder.add_root_certificate(cert.clone());
        }
        if let Some(identity) = &self.identity {
            builder.identity(identity.native.clone());
        }
        builder
            .build()
            .map_err(|err| format!("TLS setup failed: {}", error_chain(&err)))
    }

    pub fn mysql(&self) -> Result<(Option<mysql::SslOpts>, Option<Arc<TlsFiles>>), String> {
        if !self.mode.uses_tls() {
            return Ok((None, None));
        }
        let mut opts = mysql::SslOpts::default()
            .with_danger_accept_invalid_certs(!self.verifies_certificate())
            .with_danger_skip_domain_validation(!self.verifies_host());
        let mut files = TlsFiles::default();
        if let Some(ca) = &self.ca {
            let file = ca.write()?;
            opts = opts.with_root_cert_path(Some(file.to_path_buf()));
            files.ca = Some(file);
        }
        if let Some(identity) = &self.identity {
            let file = identity.write()?;
            opts = opts.with_client_identity(Some(
                mysql::ClientIdentity::new(file.to_path_buf())
                    .with_password(identity.password.clone()),
            ));
            files.identity = Some(file);
        }
        let files = (files.ca.is_some() || files.identity.is_some()).then(|| Arc::new(files));
        Ok((Some(opts), files))
    }

    pub fn explain_postgres(&self, failure: Failure) -> Failure {
        let message = &failure.message;
        if message.starts_with("error performing TLS handshake")
            && !message.contains("server does not support TLS")
        {
            self.explain(failure.message).into()
        } else {
            failure
        }
    }

    pub fn explain(&self, message: String) -> String {
        if !self.verifies_certificate() {
            return message;
        }
        let checks = if self.verifies_host() {
            "the server certificate and host name"
        } else {
            "the server certificate"
        };
        format!(
            "{message} (SSL mode {} verifies {checks}: check the CA cert, \
             or choose Require to connect without verification)",
            self.mode.label()
        )
    }
}

impl Identity {
    fn write(&self) -> Result<tempfile::TempPath, String> {
        private_file(
            "SSL client identity",
            "cooper-identity-",
            ".p12",
            &self.pkcs12,
        )
    }
}

impl Ca {
    fn write(&self) -> Result<tempfile::TempPath, String> {
        let blocks: Vec<_> = self
            .ders
            .iter()
            .map(|der| pem::Pem::new("CERTIFICATE", der.clone()))
            .collect();
        let config = pem::EncodeConfig::new().set_line_ending(pem::LineEnding::LF);
        let pem = pem::encode_many_config(&blocks, config);
        private_file("CA cert", "cooper-ca-", ".pem", pem.as_bytes())
    }
}

fn private_file(
    label: &str,
    prefix: &str,
    suffix: &str,
    data: &[u8],
) -> Result<tempfile::TempPath, String> {
    let fail = |err: std::io::Error| format!("writing the {label} file failed: {err}");
    let mut file = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .tempfile()
        .map_err(fail)?;
    file.write_all(data).map_err(fail)?;
    file.flush().map_err(fail)?;
    Ok(file.into_temp_path())
}

fn native_identity(pkcs12: &[u8], password: &str) -> Result<native_tls::Identity, String> {
    let loaded = catch_unwind(AssertUnwindSafe(|| {
        let identity =
            native_tls::Identity::from_pkcs12(pkcs12, password).map_err(|err| error_chain(&err))?;
        native_tls::TlsConnector::builder()
            .identity(identity.clone())
            .build()
            .map_err(|err| error_chain(&err))?;
        Ok(identity)
    }));
    loaded.unwrap_or_else(|_| Err("the key is damaged or of an unsupported type".into()))
}

fn read(label: &str, path: &str) -> Result<(PathBuf, Vec<u8>), String> {
    let path = expand_home(path);
    let data = fs::read(&path)
        .map_err(|err| format!("cannot read the {label} file {}: {err}", path.display()))?;
    Ok((path, data))
}

fn pem_blocks(data: &[u8]) -> Option<Vec<pem::Pem>> {
    if !data.windows(11).any(|window| window == b"-----BEGIN ") {
        return None;
    }
    pem::parse_many(data)
        .ok()
        .filter(|blocks| !blocks.is_empty())
}

fn certificate_ders(data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    match pem_blocks(data) {
        Some(blocks) => {
            let certs: Vec<_> = blocks
                .into_iter()
                .filter(|block| matches!(block.tag(), "CERTIFICATE" | "X509 CERTIFICATE"))
                .map(pem::Pem::into_contents)
                .collect();
            if certs.is_empty() {
                Err("the file holds no PEM certificate".into())
            } else {
                Ok(certs)
            }
        }
        None if data.first() == Some(&SEQUENCE) => Ok(vec![data.to_vec()]),
        None => Err("not a PEM or DER certificate".into()),
    }
}

fn ca_certificates(data: &[u8]) -> Result<Ca, String> {
    let ders = certificate_ders(data)?;
    let certs = ders
        .iter()
        .map(|der| {
            native_tls::Certificate::from_der(der)
                .map_err(|err| format!("invalid certificate: {}", error_chain(&err)))
        })
        .collect::<Result<_, _>>()?;
    Ok(Ca { ders, certs })
}

fn certificates(data: &[u8]) -> Result<Vec<Certificate>, String> {
    certificate_ders(data)?
        .iter()
        .map(|der| Certificate::from_der(der).map_err(|err| format!("invalid certificate: {err}")))
        .collect()
}

fn private_key(data: &[u8]) -> Result<PrivateKey, String> {
    let pkcs8 = match pem_blocks(data) {
        Some(blocks) => pem_private_key(&blocks)?,
        None if data.first() == Some(&SEQUENCE) => data.to_vec(),
        None => return Err("not a PEM or DER private key".into()),
    };
    PrivateKey::from_der(&pkcs8).map_err(|err| format!("invalid private key: {err}"))
}

fn pem_private_key(blocks: &[pem::Pem]) -> Result<Vec<u8>, String> {
    let encrypted = Err(
        "the key is encrypted, which is not supported; decrypt it with \
         `openssl pkey -in <key> -out <decrypted key>`"
            .to_string(),
    );
    for block in blocks {
        let legacy_encrypted = block
            .headers()
            .get("Proc-Type")
            .is_some_and(|value| value.contains("ENCRYPTED"));
        match block.tag() {
            "ENCRYPTED PRIVATE KEY" => return encrypted,
            "PRIVATE KEY" | "RSA PRIVATE KEY" | "EC PRIVATE KEY" if legacy_encrypted => {
                return encrypted;
            }
            "PRIVATE KEY" => return Ok(block.contents().to_vec()),
            "RSA PRIVATE KEY" => return Ok(rsa_to_pkcs8(block.contents())),
            "EC PRIVATE KEY" => return ec_to_pkcs8(block.contents()),
            _ => {}
        }
    }
    Err("the file holds no PEM private key".into())
}

fn rsa_to_pkcs8(pkcs1: &[u8]) -> Vec<u8> {
    pkcs8(&[RSA_ENCRYPTION, DER_NULL].concat(), pkcs1)
}

fn ec_to_pkcs8(sec1: &[u8]) -> Result<Vec<u8>, String> {
    let curve = ec_curve(sec1).ok_or("the EC key does not name its curve")?;
    Ok(pkcs8(&[EC_PUBLIC_KEY, curve].concat(), sec1))
}

fn pkcs8(algorithm: &[u8], key: &[u8]) -> Vec<u8> {
    let content = [
        &[INTEGER, 0x01, 0x00][..],
        &der(SEQUENCE, algorithm),
        &der(OCTET_STRING, key),
    ]
    .concat();
    der(SEQUENCE, &content)
}

fn ec_curve(sec1: &[u8]) -> Option<&[u8]> {
    let (SEQUENCE, mut fields, _) = read_der(sec1)? else {
        return None;
    };
    while !fields.is_empty() {
        let (tag, content, rest) = read_der(fields)?;
        if tag == EC_PARAMETERS {
            let (OBJECT_IDENTIFIER, _, after) = read_der(content)? else {
                return None;
            };
            return Some(&content[..content.len() - after.len()]);
        }
        fields = rest;
    }
    None
}

fn read_der(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = data.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > std::mem::size_of::<usize>() || rest.len() < count {
            return None;
        }
        let len = rest[..count]
            .iter()
            .fold(0usize, |len, &byte| (len << 8) | usize::from(byte));
        (len, &rest[count..])
    };
    (rest.len() >= len).then(|| (tag, &rest[..len], &rest[len..]))
}

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes: Vec<u8> = len
            .to_be_bytes()
            .into_iter()
            .skip_while(|&byte| byte == 0)
            .collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend(bytes);
    }
    out.extend_from_slice(content);
    out
}

fn random_password() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|err| {
        format!("generating a password for the SSL client identity failed: {err}")
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn pkcs12(key: PrivateKey, certs: Vec<Certificate>, password: &str) -> Result<Vec<u8>, String> {
    let mut store = KeyStore::new();
    let chain = PrivateKeyChain::new(b"cooper".as_slice(), key, certs);
    store.add_entry("cooper", KeyStoreEntry::PrivateKeyChain(chain));
    store
        .writer(password)
        .write()
        .map_err(|err| format!("building the SSL client identity failed: {err}"))
}

#[cfg(test)]
mod tests {
    use std::{
        net::{TcpListener, TcpStream},
        path::Path,
        thread,
    };

    use super::*;
    use crate::connection::{DbKind, PasswordStorage};

    fn fixture(name: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/db/fixtures")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    fn read_fixture(name: &str) -> Vec<u8> {
        fs::read(fixture(name)).unwrap()
    }

    fn conn(mode: SslMode) -> Connection {
        Connection {
            name: "tls".into(),
            kind: DbKind::Postgres,
            tag: None,
            read_only: false,
            host: "localhost".into(),
            port: None,
            user: String::new(),
            database: String::new(),
            password_storage: PasswordStorage::Keychain,
            password: String::new(),
            ssl_mode: mode,
            ssl_key: String::new(),
            ssl_cert: String::new(),
            ssl_ca: String::new(),
        }
    }

    fn with_identity(mode: SslMode, cert: &str, key: &str) -> Connection {
        Connection {
            ssl_cert: fixture(cert),
            ssl_key: fixture(key),
            ..conn(mode)
        }
    }

    fn load_err(conn: &Connection) -> String {
        match Tls::load(conn) {
            Ok(_) => panic!("{conn:?} loads"),
            Err(err) => err,
        }
    }

    #[test]
    fn a_pem_cert_and_key_become_a_pkcs12_that_native_tls_loads() {
        for (cert, key) in [
            ("client.pem", "client.key"),
            ("client.pem", "client-rsa.key"),
            ("client-ec.pem", "client-ec.key"),
        ] {
            let tls = Tls::load(&with_identity(SslMode::Require, cert, key))
                .unwrap_or_else(|err| panic!("{cert} + {key}: {err}"));
            let identity = tls.identity.as_ref().unwrap();
            assert_eq!(identity.password.len(), 32);
            native_tls::Identity::from_pkcs12(&identity.pkcs12, &identity.password).unwrap();
            tls.connector().unwrap();
        }
    }

    #[test]
    fn the_pkcs12_uses_pbes2_with_aes_and_a_sha256_mac() {
        let tls = Tls::load(&with_identity(SslMode::Require, "client.pem", "client.key")).unwrap();
        let pkcs12 = &tls.identity.unwrap().pkcs12;
        let contains = |needle: &[u8]| pkcs12.windows(needle.len()).any(|w| w == needle);
        let pbes2 = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x05, 0x0d];
        let aes_256_cbc = [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x2a];
        let sha256 = [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
        let rc2_40 = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x0c, 0x01, 0x06];
        assert!(contains(&pbes2) && contains(&aes_256_cbc) && contains(&sha256));
        assert!(!contains(&rc2_40));
    }

    #[test]
    fn pkcs1_and_sec1_keys_are_wrapped_as_pkcs8() {
        let pkcs8 = pem::parse(read_fixture("client.key")).unwrap();
        let pkcs1 = private_key(&read_fixture("client-rsa.key")).unwrap();
        assert_eq!(pkcs1.as_der(), pkcs8.contents());

        let ec = private_key(&read_fixture("client-ec.key")).unwrap();
        assert_eq!(ec.oid().to_string(), "1.2.840.10045.2.1");
        let sec1 = pem::parse_many(read_fixture("client-ec.key")).unwrap();
        assert_eq!(
            ec_curve(sec1[1].contents()),
            Some(&[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07][..])
        );
    }

    #[test]
    fn long_der_lengths_round_trip() {
        for len in [0, 1, 127, 128, 255, 256, 70_000] {
            let content = vec![7u8; len];
            let encoded = der(OCTET_STRING, &content);
            let (tag, read, rest) = read_der(&encoded).unwrap();
            assert_eq!((tag, read.len(), rest.len()), (OCTET_STRING, len, 0));
        }
        assert_eq!(read_der(&[OCTET_STRING, 0x82, 0x01]), None);
        assert_eq!(read_der(&[OCTET_STRING, 0x05, 0x00]), None);
    }

    #[test]
    fn unusable_files_are_named_in_the_error() {
        let missing = Connection {
            ssl_ca: "/nonexistent/ca.pem".into(),
            ..conn(SslMode::VerifyFull)
        };
        let err = load_err(&missing);
        assert!(
            err.starts_with("cannot read the CA cert file /nonexistent/ca.pem"),
            "{err}"
        );

        let not_a_cert = Connection {
            ssl_ca: fixture("client.key"),
            ..conn(SslMode::VerifyCa)
        };
        let err = load_err(&not_a_cert);
        assert!(
            err.contains("client.key") && err.contains("no PEM certificate"),
            "{err}"
        );

        let encrypted = with_identity(SslMode::Require, "client.pem", "client-encrypted.key");
        let err = load_err(&encrypted);
        assert!(
            err.contains("client-encrypted.key") && err.contains("encrypted"),
            "{err}"
        );

        let swapped = with_identity(SslMode::Require, "client.key", "client.pem");
        let err = load_err(&swapped);
        assert!(
            err.starts_with("SSL cert ") && err.contains("client.key"),
            "{err}"
        );

        let half = Connection {
            ssl_key: fixture("client.key"),
            ..conn(SslMode::Require)
        };
        assert_eq!(load_err(&half), "an SSL key needs an SSL cert as well");
    }

    #[test]
    fn disable_ignores_the_files() {
        let conn = Connection {
            ssl_ca: "/nonexistent/ca.pem".into(),
            ssl_key: "/nonexistent/key.pem".into(),
            ..conn(SslMode::Disable)
        };
        let tls = Tls::load(&conn).unwrap();
        assert!(tls.ca.is_none() && tls.identity.is_none());
        assert_eq!(
            tls.postgres_mode(),
            tokio_postgres::config::SslMode::Disable
        );
        let (opts, file) = tls.mysql().unwrap();
        assert!(opts.is_none() && file.is_none());
    }

    #[test]
    fn each_mode_sets_what_is_verified() {
        use tokio_postgres::config::SslMode as Pg;
        for (mode, postgres, certificate, host, fallback) in [
            (SslMode::Prefer, Pg::Prefer, false, false, true),
            (SslMode::Require, Pg::Require, false, false, false),
            (SslMode::VerifyCa, Pg::Require, true, false, false),
            (SslMode::VerifyFull, Pg::Require, true, true, false),
        ] {
            let tls = Tls::load(&conn(mode)).unwrap();
            assert_eq!(tls.postgres_mode(), postgres, "{mode:?}");
            assert_eq!(tls.verifies_certificate(), certificate, "{mode:?}");
            assert_eq!(tls.verifies_host(), host, "{mode:?}");
            assert_eq!(tls.falls_back_to_plain_text(), fallback, "{mode:?}");
            let (opts, file) = tls.mysql().unwrap();
            let opts = opts.unwrap();
            assert_eq!(opts.accept_invalid_certs(), !certificate, "{mode:?}");
            assert_eq!(opts.skip_domain_validation(), !host, "{mode:?}");
            assert!(opts.root_cert_path().is_none() && opts.client_identity().is_none());
            assert!(file.is_none());
        }
    }

    #[test]
    fn mysql_gets_private_ca_and_pkcs12_files() {
        let conn = Connection {
            ssl_ca: fixture("ca.pem"),
            ..with_identity(SslMode::VerifyFull, "client.pem", "client.key")
        };
        let tls = Tls::load(&conn).unwrap();
        let (opts, files) = tls.mysql().unwrap();
        let (opts, files) = (opts.unwrap(), files.unwrap());
        let ca = files.ca.as_deref().unwrap();
        assert_eq!(opts.root_cert_path(), Some(ca));
        assert_eq!(
            pem::parse_many(fs::read(ca).unwrap()).unwrap(),
            pem::parse_many(read_fixture("ca.pem")).unwrap()
        );
        let identity = opts.client_identity().unwrap();
        let pkcs12_path = files.identity.as_deref().unwrap();
        assert_eq!(identity.pkcs12_path(), pkcs12_path);
        let pkcs12 = fs::read(pkcs12_path).unwrap();
        native_tls::Identity::from_pkcs12(&pkcs12, identity.password().unwrap()).unwrap();
        #[cfg(unix)]
        for path in [ca, pkcs12_path] {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{}", path.display());
        }
        let paths = [ca.to_path_buf(), pkcs12_path.to_path_buf()];
        drop(files);
        assert!(
            paths.iter().all(|path| !path.exists()),
            "the files go with the connection"
        );
    }

    #[test]
    fn mysql_gets_only_the_certificates_of_a_mixed_or_der_ca_file() {
        let dir = tempfile::tempdir().unwrap();
        let mixed = dir.path().join("mixed.pem");
        let ca = String::from_utf8(read_fixture("ca.pem")).unwrap();
        let key = String::from_utf8(read_fixture("client.key")).unwrap();
        let x509 = ca.replace("CERTIFICATE-----", "X509 CERTIFICATE-----");
        fs::write(&mixed, format!("{key}{x509}{key}")).unwrap();
        let der = dir.path().join("ca.der");
        fs::write(&der, pem::parse(ca.as_bytes()).unwrap().contents()).unwrap();
        for path in [&mixed, &der] {
            let conn = Connection {
                ssl_ca: path.to_string_lossy().into_owned(),
                ..conn(SslMode::VerifyCa)
            };
            let tls = Tls::load(&conn).unwrap();
            let (opts, files) = tls.mysql().unwrap();
            let file = opts.unwrap().root_cert_path().unwrap().to_path_buf();
            assert_eq!(
                pem::parse_many(fs::read(&file).unwrap()).unwrap(),
                pem::parse_many(ca.as_bytes()).unwrap(),
                "{}",
                path.display()
            );
            drop(files);
        }
    }

    #[test]
    fn a_cert_and_key_that_do_not_belong_together_are_named() {
        let dir = tempfile::tempdir().unwrap();
        let damaged = dir.path().join("damaged.key");
        let body = pem::Pem::new("RSA PRIVATE KEY", vec![0x30, 0x03, 0x02, 0x01, 0x00]);
        fs::write(&damaged, pem::encode(&body)).unwrap();
        let damaged = damaged.to_string_lossy().into_owned();
        for (cert, key) in [
            (fixture("client.pem"), fixture("client-ec.key")),
            (fixture("client-ec.pem"), fixture("client.key")),
            (fixture("client.pem"), damaged),
        ] {
            let conn = Connection {
                ssl_cert: cert.clone(),
                ssl_key: key.clone(),
                ..conn(SslMode::Require)
            };
            let err = load_err(&conn);
            assert!(
                err.starts_with(&format!(
                    "SSL cert {cert} and SSL key {key} cannot be used together: "
                )),
                "{err}"
            );
        }
    }

    #[test]
    fn verification_failures_explain_the_ssl_mode() {
        let tls = Tls::load(&conn(SslMode::VerifyFull)).unwrap();
        let message = tls.explain("certificate verify failed".into());
        assert_eq!(
            message,
            "certificate verify failed (SSL mode Verify Full verifies the server certificate \
             and host name: check the CA cert, or choose Require to connect without verification)"
        );
        let handshake = tls.explain_postgres(Failure::from(
            "error performing TLS handshake: certificate verify failed",
        ));
        assert!(
            handshake.message.contains("SSL mode Verify Full"),
            "{handshake:?}"
        );
        for message in [
            "error performing TLS handshake: server does not support TLS",
            "password authentication failed for user \"app\"",
        ] {
            assert_eq!(tls.explain_postgres(message.into()).message, message);
        }
        let tls = Tls::load(&conn(SslMode::VerifyCa)).unwrap();
        assert!(
            tls.explain("x".into())
                .contains("Verify CA verifies the server certificate:")
        );
        let tls = Tls::load(&conn(SslMode::Require)).unwrap();
        assert_eq!(tls.explain("x".into()), "x");
    }

    fn serve_tls(times: usize) -> u16 {
        let cert = certificates(&read_fixture("server.pem")).unwrap();
        let key = private_key(&read_fixture("server.key")).unwrap();
        let identity = pkcs12(key, cert, "server").unwrap();
        let identity = native_tls::Identity::from_pkcs12(&identity, "server").unwrap();
        let acceptor = native_tls::TlsAcceptor::new(identity).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming().take(times) {
                let Ok(stream) = stream else { continue };
                if let Ok(mut tls) = acceptor.accept(stream) {
                    let _ = tls.write_all(b"ok");
                    let _ = tls.shutdown();
                }
            }
        });
        port
    }

    fn handshake(conn: &Connection, port: u16, domain: &str) -> Result<(), String> {
        let connector = Tls::load(conn)?.connector()?;
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        connector
            .connect(domain, stream)
            .map(drop)
            .map_err(|err| err.to_string())
    }

    #[test]
    fn the_connector_verifies_what_the_mode_asks_for() {
        let with_ca = |mode| Connection {
            ssl_ca: fixture("ca.pem"),
            ..conn(mode)
        };
        let cases = [
            (conn(SslMode::Require), "other.invalid", true),
            (conn(SslMode::VerifyCa), "localhost", false),
            (conn(SslMode::VerifyFull), "localhost", false),
            (with_ca(SslMode::VerifyCa), "localhost", true),
            (with_ca(SslMode::VerifyCa), "other.invalid", true),
            (with_ca(SslMode::VerifyFull), "localhost", true),
            (with_ca(SslMode::VerifyFull), "other.invalid", false),
        ];
        let port = serve_tls(cases.len());
        for (conn, domain, ok) in cases {
            let result = handshake(&conn, port, domain);
            assert_eq!(
                result.is_ok(),
                ok,
                "{:?} to {domain}: {result:?}",
                conn.ssl_mode
            );
        }
    }
}
