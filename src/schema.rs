use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::{Error, Result};

pub const RECIPE_SCHEMA: &str = "uci-grabber-recipe/v1";
pub const CATALOG_SCHEMA: &str = "uci-grabber-catalog/v1";
pub const MAX_MANIFEST_BYTES: usize = 512 * 1024;
pub const MAX_SIGNATURE_BYTES: usize = 4 * 1024;
pub const MAX_PACKAGE_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub publisher: Publisher,
    pub license: License,
    pub homepage: String,
    pub minimum_fisheye_version: String,
    pub models: Vec<Model>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publisher {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct License {
    pub spdx: String,
    pub name: String,
    pub url: String,
    pub source_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub description: String,
    pub packages: Vec<Package>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub platform: String,
    pub artifacts: Vec<Artifact>,
    pub executable: String,
    pub working_directory: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub url: String,
    pub byte_count: u64,
    pub sha256: String,
    pub format: ArchiveFormat,
    pub destination: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    Runtime,
    Model,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchiveFormat {
    #[serde(rename = "raw")]
    Raw,
    #[serde(rename = "zip")]
    Zip,
    #[serde(rename = "tar.gz")]
    TarGz,
    #[serde(rename = "tar.zst")]
    TarZst,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema: String,
    pub generated_at: String,
    pub expires_at: String,
    pub recipes: Vec<Recipe>,
}

impl Recipe {
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::InvalidRecipe(format!(
                "manifest exceeds {MAX_MANIFEST_BYTES} bytes"
            )));
        }
        let recipe: Self = serde_json::from_slice(bytes)?;
        recipe.validate()?;
        Ok(recipe)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != RECIPE_SCHEMA {
            return invalid(format!("unsupported schema `{}`", self.schema));
        }
        validate_slug(&self.id, "recipe id")?;
        validate_text(&self.name, "name", 256)?;
        validate_version(&self.version)?;
        validate_text(&self.description, "description", 4_096)?;
        validate_text(&self.publisher.name, "publisher name", 256)?;
        validate_https(&self.publisher.url, "publisher URL")?;
        validate_spdx(&self.license.spdx)?;
        validate_text(&self.license.name, "license name", 256)?;
        validate_https(&self.license.url, "license URL")?;
        validate_https(&self.license.source_url, "source URL")?;
        validate_https(&self.homepage, "homepage")?;
        validate_core_version(&self.minimum_fisheye_version, "minimum_fisheye_version")?;
        if self.models.is_empty() {
            return invalid("at least one model is required");
        }
        if self.models.len() > 64 {
            return invalid("models contains more than 64 entries");
        }

        let mut model_ids = BTreeSet::new();
        for model in &self.models {
            validate_slug(&model.id, "model id")?;
            validate_text(&model.name, "model name", 256)?;
            validate_text(&model.description, "model description", 4_096)?;
            if !model_ids.insert(&model.id) {
                return invalid(format!("duplicate model id `{}`", model.id));
            }
            if model.packages.is_empty() {
                return invalid(format!("model `{}` has no platform package", model.id));
            }
            if model.packages.len() > 6 {
                return invalid(format!("model `{}` has more than 6 packages", model.id));
            }
            let mut platforms = BTreeSet::new();
            for package in &model.packages {
                validate_platform(&package.platform)?;
                if !platforms.insert(&package.platform) {
                    return invalid(format!(
                        "model `{}` has duplicate platform `{}`",
                        model.id, package.platform
                    ));
                }
                validate_relative(&package.executable, false, "executable")?;
                validate_relative(&package.working_directory, true, "working directory")?;
                if package.artifacts.is_empty() {
                    return invalid(format!(
                        "model `{}` package `{}` has no artifacts",
                        model.id, package.platform
                    ));
                }
                if package.artifacts.len() > 16 {
                    return invalid(format!(
                        "model `{}` package `{}` has more than 16 artifacts",
                        model.id, package.platform
                    ));
                }
                let mut destinations = BTreeSet::new();
                let mut runtime_count = 0_usize;
                let mut model_count = 0_usize;
                let mut download_bytes = 0_u64;
                for artifact in &package.artifacts {
                    validate_https(&artifact.url, "artifact URL")?;
                    if artifact.byte_count == 0 {
                        return invalid("artifact byte_count must be greater than zero");
                    }
                    if artifact.byte_count > 1_073_741_824 {
                        return invalid("artifact byte_count exceeds 1073741824");
                    }
                    if artifact.kind == ArtifactKind::Model && artifact.byte_count > 419_430_400 {
                        return invalid("model artifact byte_count exceeds 419430400");
                    }
                    download_bytes =
                        download_bytes
                            .checked_add(artifact.byte_count)
                            .ok_or_else(|| {
                                Error::InvalidRecipe(
                                    "package artifact byte_count total overflowed".into(),
                                )
                            })?;
                    if download_bytes > MAX_PACKAGE_DOWNLOAD_BYTES {
                        return invalid(format!(
                            "model `{}` package `{}` declares more than {MAX_PACKAGE_DOWNLOAD_BYTES} download bytes",
                            model.id, package.platform
                        ));
                    }
                    runtime_count += usize::from(artifact.kind == ArtifactKind::Runtime);
                    model_count += usize::from(artifact.kind == ArtifactKind::Model);
                    validate_digest(&artifact.sha256)?;
                    validate_relative(&artifact.destination, false, "artifact destination")?;
                    if !destinations.insert(&artifact.destination) {
                        return invalid(format!(
                            "duplicate artifact destination `{}`",
                            artifact.destination
                        ));
                    }
                }
                let expected_working_directory = package
                    .executable
                    .rsplit_once('/')
                    .map_or(".", |(parent, _)| parent);
                if package.working_directory != expected_working_directory {
                    return invalid(format!(
                        "model `{}` package `{}` working_directory must equal executable parent `{expected_working_directory}`",
                        model.id, package.platform
                    ));
                }
                if runtime_count != 1 {
                    return invalid(format!(
                        "model `{}` package `{}` must contain exactly one runtime artifact",
                        model.id, package.platform
                    ));
                }
                if model_count > 1 {
                    return invalid(format!(
                        "model `{}` package `{}` contains multiple model artifacts",
                        model.id, package.platform
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn model(&self, id: &str) -> Result<&Model> {
        self.models
            .iter()
            .find(|model| model.id == id)
            .ok_or_else(|| Error::InvalidRecipe(format!("unknown model `{id}`")))
    }
}

impl Catalog {
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::InvalidCatalog(format!(
                "catalog exceeds {MAX_MANIFEST_BYTES} bytes"
            )));
        }
        let catalog: Self = serde_json::from_slice(bytes)?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != CATALOG_SCHEMA {
            return Err(Error::InvalidCatalog(format!(
                "unsupported schema `{}`",
                self.schema
            )));
        }
        if self.generated_at.is_empty() || self.expires_at.is_empty() {
            return Err(Error::InvalidCatalog(
                "generated_at and expires_at are required".into(),
            ));
        }
        let generated = canonical_timestamp(&self.generated_at, "generated_at")?;
        let expires = canonical_timestamp(&self.expires_at, "expires_at")?;
        if expires <= generated {
            return Err(Error::InvalidCatalog(
                "expires_at must be later than generated_at".into(),
            ));
        }
        if self.recipes.len() > 1_024 {
            return Err(Error::InvalidCatalog(
                "catalog contains more than 1024 recipes".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        for recipe in &self.recipes {
            recipe.validate().map_err(|error| {
                Error::InvalidCatalog(format!("recipe `{}`: {error}", recipe.id))
            })?;
            if !ids.insert(&recipe.id) {
                return Err(Error::InvalidCatalog(format!(
                    "duplicate recipe id `{}`",
                    recipe.id
                )));
            }
        }
        Ok(())
    }

    pub fn ensure_not_expired(&self) -> Result<()> {
        canonical_timestamp(&self.expires_at, "expires_at")?;
        let now = jiff::Timestamp::now()
            .strftime("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        if self.expires_at.as_str() <= now.as_str() {
            Err(Error::InvalidCatalog(format!(
                "catalog expired at {}",
                self.expires_at
            )))
        } else {
            Ok(())
        }
    }
}

pub fn current_platform() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        ("macos", "aarch64") => Ok("macos-aarch64"),
        ("windows", "x86_64") => Ok("windows-x86_64"),
        ("windows", "aarch64") => Ok("windows-aarch64"),
        (os, arch) => Err(Error::UnsupportedPlatform(format!("{os}-{arch}"))),
    }
}

fn validate_platform(platform: &str) -> Result<()> {
    const PLATFORMS: [&str; 6] = [
        "linux-x86_64",
        "linux-aarch64",
        "macos-x86_64",
        "macos-aarch64",
        "windows-x86_64",
        "windows-aarch64",
    ];
    if PLATFORMS.contains(&platform) {
        Ok(())
    } else {
        invalid(format!("unknown platform `{platform}`"))
    }
}

fn validate_slug(value: &str, label: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.chars().count() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && !value.as_bytes().windows(2).any(|pair| pair == b"--");
    if valid {
        Ok(())
    } else {
        invalid(format!("{label} must be a lowercase ASCII slug"))
    }
}

fn validate_text(value: &str, label: &str, max_length: usize) -> Result<()> {
    if value.chars().count() > max_length
        || !value.chars().any(|character| !character.is_whitespace())
        || value
            .chars()
            .any(|character| character <= '\u{1f}' || character == '\u{7f}')
    {
        invalid(format!("{label} is empty or invalid"))
    } else {
        Ok(())
    }
}

fn validate_version(version: &str) -> Result<()> {
    if version.chars().count() > 80 || !valid_semver(version, true) {
        invalid("version contains unsupported characters")
    } else {
        Ok(())
    }
}

fn validate_https(value: &str, label: &str) -> Result<()> {
    let parsed = url::Url::parse(value).ok();
    if value.chars().count() > 2_048
        || value
            .chars()
            .any(|character| character <= '\u{1f}' || character == '\u{7f}')
        || parsed.as_ref().is_none_or(|parsed| {
            parsed.scheme() != "https"
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
        })
    {
        invalid(format!("{label} must be an HTTPS URL"))
    } else {
        Ok(())
    }
}

fn validate_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        invalid("sha256 must contain exactly 64 hexadecimal characters")
    } else {
        Ok(())
    }
}

fn validate_relative(value: &str, allow_dot: bool, label: &str) -> Result<()> {
    if value.chars().count() > 512 || !is_portable_relative_path(value, allow_dot) {
        invalid(format!("{label} is not a safe relative path"))
    } else {
        Ok(())
    }
}

pub(crate) fn is_portable_relative_path(value: &str, allow_dot: bool) -> bool {
    if value == "." {
        return allow_dot;
    }
    !value.is_empty()
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains('\\')
        && !value.contains("//")
        && value.split('/').all(is_portable_path_component)
}

fn is_portable_path_component(component: &str) -> bool {
    if component.is_empty()
        || matches!(component, "." | "..")
        || component.ends_with('.')
        || component.ends_with(' ')
        || component.chars().any(|character| {
            character <= '\u{1f}'
                || character == '\u{7f}'
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
    {
        return false;
    }

    let stem = component.split('.').next().unwrap_or_default();
    let stem = stem.to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) && !is_numbered_windows_device(&stem, "COM")
        && !is_numbered_windows_device(&stem, "LPT")
}

fn is_numbered_windows_device(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix).is_some_and(|suffix| {
        matches!(
            suffix,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
    })
}

fn validate_spdx(value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.chars().count() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'));
    if valid {
        Ok(())
    } else {
        invalid("license spdx is invalid")
    }
}

fn validate_core_version(value: &str, label: &str) -> Result<()> {
    if value.chars().count() <= 64 && valid_semver(value, false) {
        Ok(())
    } else {
        invalid(format!("{label} must be a three-component version"))
    }
}

fn valid_semver(value: &str, allow_suffix: bool) -> bool {
    let (without_build, build) = match value.split_once('+') {
        Some((left, right)) if !right.contains('+') => (left, Some(right)),
        Some(_) => return false,
        None => (value, None),
    };
    let (core, prerelease) = match without_build.split_once('-') {
        Some((left, right)) => (left, Some(right)),
        None => (without_build, None),
    };
    let mut components = core.split('.');
    let valid_component = |component: &str| {
        !component.is_empty()
            && component.bytes().all(|byte| byte.is_ascii_digit())
            && (component == "0" || !component.starts_with('0'))
    };
    valid_component(components.next().unwrap_or_default())
        && valid_component(components.next().unwrap_or_default())
        && valid_component(components.next().unwrap_or_default())
        && components.next().is_none()
        && if allow_suffix {
            [prerelease, build]
                .into_iter()
                .flatten()
                .all(valid_semver_suffix)
        } else {
            prerelease.is_none() && build.is_none()
        }
}

fn valid_semver_suffix(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn canonical_timestamp(value: &str, label: &str) -> Result<jiff::civil::DateTime> {
    let bytes = value.as_bytes();
    let canonical_shape = bytes.len() == 20
        && matches!(bytes.get(4), Some(b'-'))
        && matches!(bytes.get(7), Some(b'-'))
        && matches!(bytes.get(10), Some(b'T'))
        && matches!(bytes.get(13), Some(b':'))
        && matches!(bytes.get(16), Some(b':'))
        && matches!(bytes.get(19), Some(b'Z'))
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
        });
    if !canonical_shape {
        return Err(Error::InvalidCatalog(format!(
            "{label} must be canonical UTC RFC 3339"
        )));
    }
    value[..19]
        .parse::<jiff::civil::DateTime>()
        .map_err(|error| {
            Error::InvalidCatalog(format!("{label} must be canonical UTC RFC 3339: {error}"))
        })
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::InvalidRecipe(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_recipe() -> Recipe {
        Recipe {
            schema: RECIPE_SCHEMA.into(),
            id: "test-engine".into(),
            name: "Test Engine".into(),
            version: "1.0.0".into(),
            description: "Fixture".into(),
            publisher: Publisher {
                name: "Fixture".into(),
                url: "https://example.test".into(),
            },
            license: License {
                spdx: "MIT".into(),
                name: "MIT".into(),
                url: "https://example.test/license".into(),
                source_url: "https://example.test/source".into(),
            },
            homepage: "https://example.test/engine".into(),
            minimum_fisheye_version: "1.7.0".into(),
            models: vec![Model {
                id: "small".into(),
                name: "Small".into(),
                description: "Small fixture".into(),
                packages: vec![Package {
                    platform: "linux-x86_64".into(),
                    artifacts: vec![Artifact {
                        kind: ArtifactKind::Runtime,
                        url: "https://example.test/engine.zip".into(),
                        byte_count: 42,
                        sha256: "a".repeat(64),
                        format: ArchiveFormat::Zip,
                        destination: "runtime".into(),
                    }],
                    executable: "runtime/engine".into(),
                    working_directory: "runtime".into(),
                }],
            }],
        }
    }

    #[test]
    fn valid_recipe_is_accepted() {
        valid_recipe().validate().unwrap();
    }

    #[test]
    fn traversal_is_rejected() {
        let mut recipe = valid_recipe();
        recipe.models[0].packages[0].artifacts[0].destination = "../escape".into();
        assert!(recipe.validate().is_err());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let mut value = serde_json::to_value(valid_recipe()).unwrap();
        value["hook"] = serde_json::json!("echo unsafe");
        assert!(Recipe::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn shared_schema_fixture_is_accepted() {
        Recipe::from_json(include_bytes!(
            "../catalog/tests/fixtures/valid-recipe.json"
        ))
        .unwrap();
    }

    #[test]
    fn rejects_recipe_contract_edges() {
        let mut recipe = valid_recipe();
        recipe.id = "has.dot".into();
        assert!(recipe.validate().is_err());
        let mut recipe = valid_recipe();
        recipe.models[0].packages[0].artifacts[0].kind = ArtifactKind::Model;
        assert!(recipe.validate().is_err());
        let mut recipe = valid_recipe();
        recipe.publisher.url = "https://user@example.test".into();
        assert!(recipe.validate().is_err());
    }

    #[test]
    fn rejects_windows_nonportable_recipe_paths() {
        for path in [
            "runtime/engine:stream",
            "runtime/bad<name",
            "runtime/bad>name",
            "runtime/bad\"name",
            "runtime/bad|name",
            "runtime/bad?name",
            "runtime/bad*name",
            "runtime/trailing.",
            "runtime/trailing ",
            "runtime/CON",
            "runtime/aux.txt",
            "runtime/CoM1.bin",
            "runtime/lPt9",
            "runtime/COM¹.log",
            "runtime/CONIN$.txt",
            "runtime/CLOCK$.cfg",
            "runtime/./engine",
            "runtime/../engine",
            "runtime/",
            "C:/engine",
        ] {
            assert!(
                !is_portable_relative_path(path, false),
                "unexpectedly accepted {path}"
            );
            let mut recipe = valid_recipe();
            recipe.models[0].packages[0].artifacts[0].destination = path.into();
            assert!(recipe.validate().is_err(), "unexpectedly accepted {path}");
        }

        assert!(is_portable_relative_path("runtime/engine.exe", false));
        assert!(is_portable_relative_path("weights/maia 5m.bin", false));
        assert!(is_portable_relative_path(".", true));
        assert!(!is_portable_relative_path(".", false));
    }

    #[test]
    fn rejects_excessive_cumulative_package_downloads() {
        let mut recipe = valid_recipe();
        let package = &mut recipe.models[0].packages[0];
        package.artifacts[0].byte_count = 1024 * 1024 * 1024;
        for (destination, byte_count) in [("support-one", 1024 * 1024 * 1024), ("support-two", 1)] {
            package.artifacts.push(Artifact {
                kind: ArtifactKind::Other,
                url: format!("https://example.test/{destination}"),
                byte_count,
                sha256: "b".repeat(64),
                format: ArchiveFormat::Raw,
                destination: destination.into(),
            });
        }

        let error = recipe.validate().unwrap_err();
        assert!(error.to_string().contains("download bytes"));
    }

    #[test]
    fn requires_working_directory_to_match_executable_parent() {
        let mut recipe = valid_recipe();
        recipe.models[0].packages[0].working_directory = ".".into();

        let error = recipe.validate().unwrap_err();
        assert!(error.to_string().contains("executable parent"));
    }
}
