use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use ring::signature::{ED25519, UnparsedPublicKey};

use crate::schema::{Catalog, MAX_MANIFEST_BYTES, MAX_SIGNATURE_BYTES};
use crate::{Error, Result};

pub const VERIFIED_CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub const DEFAULT_CATALOG_URL: &str = concat!(
    "https://github.com/EnchiladaBoy/UCI-Grabber/releases/download/v",
    env!("CARGO_PKG_VERSION"),
    "/catalog-v",
    env!("CARGO_PKG_VERSION"),
    ".json"
);
pub const DEFAULT_SIGNATURE_URL: &str = concat!(
    "https://github.com/EnchiladaBoy/UCI-Grabber/releases/download/v",
    env!("CARGO_PKG_VERSION"),
    "/catalog-v",
    env!("CARGO_PKG_VERSION"),
    ".sig"
);

#[derive(Clone, Debug)]
pub struct VerifiedCatalog {
    pub catalog: Catalog,
    pub exact_bytes: Vec<u8>,
    pub signature: [u8; 64],
}

impl VerifiedCatalog {
    /// Verifies the detached Ed25519 signature over the exact JSON bytes, then parses it.
    pub fn verify(catalog_bytes: &[u8], signature_bytes: &[u8], public_key: &[u8]) -> Result<Self> {
        Self::verify_with_policy(catalog_bytes, signature_bytes, public_key, false)
    }

    pub fn verify_bootstrap(
        catalog_bytes: &[u8],
        signature_bytes: &[u8],
        public_key: &[u8],
    ) -> Result<Self> {
        Self::verify_with_policy(catalog_bytes, signature_bytes, public_key, true)
    }

    fn verify_with_policy(
        catalog_bytes: &[u8],
        signature_bytes: &[u8],
        public_key: &[u8],
        bootstrap: bool,
    ) -> Result<Self> {
        if catalog_bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::InvalidCatalog(format!(
                "catalog exceeds {MAX_MANIFEST_BYTES} bytes"
            )));
        }
        if signature_bytes.len() > MAX_SIGNATURE_BYTES {
            return Err(Error::InvalidSignature);
        }
        if public_key.len() != 32 {
            return Err(Error::InvalidCatalog(
                "Ed25519 public key must be 32 bytes".into(),
            ));
        }
        let signature = decode_signature(signature_bytes)?;
        UnparsedPublicKey::new(&ED25519, public_key)
            .verify(catalog_bytes, &signature)
            .map_err(|_| Error::InvalidSignature)?;
        let catalog = Catalog::from_json(catalog_bytes)?;
        catalog.ensure_not_expired()?;
        if bootstrap && !catalog.recipes.is_empty() {
            return Err(Error::InvalidCatalog(
                "long-lived bootstrap catalog must remain empty".into(),
            ));
        }
        Ok(Self {
            catalog,
            exact_bytes: catalog_bytes.to_vec(),
            signature,
        })
    }
}

pub fn bundled_catalog() -> Result<VerifiedCatalog> {
    let public_key = bundled_public_key()?;
    VerifiedCatalog::verify(
        include_bytes!("../catalog/catalog.json"),
        include_bytes!("../catalog/catalog.sig"),
        &public_key,
    )
}

pub fn bundled_public_key() -> Result<[u8; 32]> {
    decode_public_key(include_bytes!("../catalog/catalog.pub"))
}

pub fn default_client(cache: CatalogCache) -> Result<CatalogClient> {
    Ok(CatalogClient::new(bundled_public_key()?, cache))
}

pub fn preferred_catalog(client: &CatalogClient) -> Result<VerifiedCatalog> {
    match client.cached()? {
        Some(catalog) => Ok(catalog),
        None => bundled_catalog(),
    }
}

#[derive(Clone, Debug)]
pub struct CatalogCache {
    directory: PathBuf,
}

impl CatalogCache {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn store(&self, verified: &VerifiedCatalog) -> Result<()> {
        fs::create_dir_all(&self.directory).map_err(|source| Error::io(&self.directory, source))?;
        atomic_write(&self.directory.join("catalog.json"), &verified.exact_bytes)?;
        atomic_write(&self.directory.join("catalog.sig"), &verified.signature)?;
        Ok(())
    }

    /// Loads a cached catalog only while its file is at most 24 hours old, and always
    /// re-verifies the detached signature before returning it.
    pub fn load_fresh(&self, public_key: &[u8]) -> Result<Option<VerifiedCatalog>> {
        let catalog_path = self.directory.join("catalog.json");
        let signature_path = self.directory.join("catalog.sig");
        let metadata = match fs::metadata(&catalog_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(Error::io(&catalog_path, source)),
        };
        let modified = metadata
            .modified()
            .map_err(|source| Error::io(&catalog_path, source))?;
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        if age > VERIFIED_CACHE_MAX_AGE {
            return Ok(None);
        }
        let catalog = read_bounded(&catalog_path, MAX_MANIFEST_BYTES)?;
        let signature = read_bounded(&signature_path, MAX_SIGNATURE_BYTES)?;
        VerifiedCatalog::verify(&catalog, &signature, public_key).map(Some)
    }
}

#[derive(Clone, Debug)]
pub struct CatalogClient {
    pub catalog_url: String,
    pub signature_url: String,
    pub public_key: [u8; 32],
    pub cache: CatalogCache,
}

impl CatalogClient {
    pub fn new(public_key: [u8; 32], cache: CatalogCache) -> Self {
        Self {
            catalog_url: DEFAULT_CATALOG_URL.into(),
            signature_url: DEFAULT_SIGNATURE_URL.into(),
            public_key,
            cache,
        }
    }

    /// Explicitly refreshes the network catalog. A failed fetch or signature check
    /// never replaces the last verified cache.
    pub fn refresh(&self) -> Result<VerifiedCatalog> {
        let catalog = fetch_bounded(&self.catalog_url, MAX_MANIFEST_BYTES)?;
        let signature = fetch_bounded(&self.signature_url, MAX_SIGNATURE_BYTES)?;
        let verified = VerifiedCatalog::verify(&catalog, &signature, &self.public_key)?;
        self.cache.store(&verified)?;
        Ok(verified)
    }

    pub fn cached(&self) -> Result<Option<VerifiedCatalog>> {
        self.cache.load_fresh(&self.public_key)
    }
}

pub fn decode_public_key(bytes: &[u8]) -> Result<[u8; 32]> {
    let decoded = decode_fixed::<32>(bytes).map_err(|()| {
        Error::InvalidCatalog("public key must be raw, hex, or base64 Ed25519 bytes".into())
    })?;
    Ok(decoded)
}

fn decode_signature(bytes: &[u8]) -> Result<[u8; 64]> {
    decode_fixed::<64>(bytes).map_err(|()| Error::InvalidSignature)
}

fn decode_fixed<const N: usize>(bytes: &[u8]) -> std::result::Result<[u8; N], ()> {
    if bytes.len() == N {
        return bytes.try_into().map_err(|_| ());
    }
    let trimmed = trim_ascii(bytes);
    if trimmed.len() == N * 2 && trimmed.iter().all(u8::is_ascii_hexdigit) {
        let mut decoded = [0_u8; N];
        for (index, pair) in trimmed.chunks_exact(2).enumerate() {
            let text = std::str::from_utf8(pair).map_err(|_| ())?;
            decoded[index] = u8::from_str_radix(text, 16).map_err(|_| ())?;
        }
        return Ok(decoded);
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .map_err(|_| ())?;
    decoded.try_into().map_err(|_| ())
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path).map_err(|source| Error::io(path, source))?;
    if metadata.len() > limit as u64 {
        return Err(Error::Other(format!(
            "{} exceeds {limit} bytes",
            path.display()
        )));
    }
    fs::read(path).map_err(|source| Error::io(path, source))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension("next");
    if temporary.exists() {
        fs::remove_file(&temporary).map_err(|source| Error::io(&temporary, source))?;
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|source| Error::io(&temporary, source))?;
    file.write_all(bytes)
        .map_err(|source| Error::io(&temporary, source))?;
    file.sync_all()
        .map_err(|source| Error::io(&temporary, source))?;
    if path.exists() {
        fs::remove_file(path).map_err(|source| Error::io(path, source))?;
    }
    fs::rename(&temporary, path).map_err(|source| Error::io(path, source))
}

fn fetch_bounded(url: &str, limit: usize) -> Result<Vec<u8>> {
    if !url.starts_with("https://") {
        return Err(Error::Download {
            url: url.into(),
            message: "catalog endpoints must use HTTPS".into(),
        });
    }
    let config = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .max_redirects_will_error(true)
        .timeout_global(Some(Duration::from_secs(15)))
        .build();
    let agent: ureq::Agent = config.into();
    let mut response = agent
        .get(url)
        .header(
            "User-Agent",
            concat!("UCI-Grabber/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|source| Error::Download {
            url: url.into(),
            message: source.to_string(),
        })?;
    let mut reader = response.body_mut().as_reader().take(limit as u64 + 1);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut bytes).map_err(|source| Error::Download {
        url: url.into(),
        message: source.to_string(),
    })?;
    if bytes.len() > limit {
        return Err(Error::DownloadTooLarge {
            limit: limit as u64,
        });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use ring::signature::{Ed25519KeyPair, KeyPair as _};

    use super::*;

    #[test]
    fn verifies_exact_catalog_bytes() {
        let random = ring::rand::SystemRandom::new();
        let key = Ed25519KeyPair::generate_pkcs8(&random).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(key.as_ref()).unwrap();
        let bytes = br#"{"schema":"uci-grabber-catalog/v1","generated_at":"2026-07-21T00:00:00Z","expires_at":"9999-12-31T23:59:59Z","recipes":[]}"#;
        let signature = pair.sign(bytes);
        let verified =
            VerifiedCatalog::verify(bytes, signature.as_ref(), pair.public_key().as_ref()).unwrap();
        assert!(verified.catalog.recipes.is_empty());
        assert!(
            VerifiedCatalog::verify(b"{}", signature.as_ref(), pair.public_key().as_ref()).is_err()
        );
    }

    #[test]
    fn bundled_catalog_signature_is_valid() {
        bundled_catalog().unwrap();
        assert!(DEFAULT_CATALOG_URL.contains(env!("CARGO_PKG_VERSION")));
        assert!(!DEFAULT_CATALOG_URL.contains("/latest/"));
    }

    #[test]
    fn accepts_hex_signature_and_key() {
        let key = [0x12_u8; 32];
        assert_eq!(
            decode_public_key(format!("{}\n", hex(&key)).as_bytes()).unwrap(),
            key
        );
        let signature = [0x34_u8; 64];
        assert_eq!(
            decode_signature(hex(&signature).as_bytes()).unwrap(),
            signature
        );
    }

    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;

        bytes.iter().fold(
            String::with_capacity(bytes.len() * 2),
            |mut output, byte| {
                write!(output, "{byte:02x}").unwrap();
                output
            },
        )
    }
}
