use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::download::sha256_file;
use crate::extract::{MAX_ARCHIVE_ENTRIES, MAX_ARCHIVE_ENTRY_BYTES, MAX_EXTRACTED_BYTES};
use crate::{Error, Result};

pub const REGISTRY_SCHEMA: &str = "uci-grabber-registry/v1";
pub(crate) const INSTALL_RECORD_FILE: &str = "uci-grabber-install.json";
const PORTABLE_MARKER: &str = "portable.flag";
const PORTABLE_DATA_DIRECTORY: &str = "UCI-Grabber-Data";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub schema: String,
    pub installs: Vec<InstallRecord>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            schema: REGISTRY_SCHEMA.into(),
            installs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRecord {
    pub install_id: String,
    pub recipe_id: String,
    pub recipe_sha256: String,
    pub model_id: String,
    pub name: String,
    pub version: String,
    pub platform: String,
    pub generation_root: PathBuf,
    pub executable: PathBuf,
    pub executable_sha256: String,
    pub package_sha256: String,
    pub package_file_count: u64,
    pub package_byte_count: u64,
    pub working_directory: PathBuf,
    pub source: InstallSource,
    pub installed_at_unix: u64,
    pub publisher: String,
    pub license_spdx: String,
    pub license_url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallSource {
    Curated,
    UnreviewedRecipe,
}

impl fmt::Display for InstallSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Curated => "Curated",
            Self::UnreviewedRecipe => "Unreviewed",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Integrity {
    Verified,
    Missing,
    Changed { expected: String, actual: String },
}

#[derive(Clone, Debug)]
pub struct RegistryStore {
    data_root: PathBuf,
    portable: bool,
}

impl RegistryStore {
    pub fn open_default() -> Result<Self> {
        if let Ok(executable) = std::env::current_exe()
            && let Some(data_root) = portable_data_root(&executable)
        {
            let store = Self::new_portable(data_root);
            store.ensure_writable()?;
            return Ok(store);
        }
        let directories = ProjectDirs::from("dev", "EnchiladaBoy", "UCI Grabber")
            .ok_or_else(|| Error::Other("could not determine application data directory".into()))?;
        Ok(Self::new(directories.data_dir()))
    }

    pub fn new(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
            portable: false,
        }
    }

    pub(crate) fn new_portable(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
            portable: true,
        }
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn installs_dir(&self) -> PathBuf {
        self.data_root.join("installs")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.data_root.join("catalog-cache")
    }

    pub fn registry_path(&self) -> PathBuf {
        self.data_root.join("registry.json")
    }

    fn ensure_writable(&self) -> Result<()> {
        fs::create_dir_all(&self.data_root).map_err(|source| {
            Error::Other(if self.portable {
                format!(
                    "portable data folder {} is not writable: {source}; extract or move UCI Grabber to a writable folder",
                    self.data_root.display()
                )
            } else {
                format!(
                    "application data folder {} is not writable: {source}",
                    self.data_root.display()
                )
            })
        })?;
        tempfile::NamedTempFile::new_in(&self.data_root)
            .and_then(tempfile::NamedTempFile::close)
            .map_err(|source| {
                Error::Other(if self.portable {
                    format!(
                        "portable data folder {} is not writable: {source}; extract or move UCI Grabber to a writable folder",
                        self.data_root.display()
                    )
                } else {
                    format!(
                        "application data folder {} is not writable: {source}",
                        self.data_root.display()
                    )
                })
            })
    }

    pub fn load(&self) -> Result<Registry> {
        let path = self.registry_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let backup = path.with_extension("backup");
                match fs::read(&backup) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(Registry::default());
                    }
                    Err(source) => return Err(Error::io(backup, source)),
                }
            }
            Err(source) => return Err(Error::io(path, source)),
        };
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(Error::Other("registry exceeds 8 MiB safety limit".into()));
        }
        let mut registry: Registry = serde_json::from_slice(&bytes)?;
        if registry.schema != REGISTRY_SCHEMA {
            return Err(Error::Other(format!(
                "unsupported registry schema `{}`",
                registry.schema
            )));
        }
        for record in &mut registry.installs {
            self.rebase_portable_record(record)?;
        }
        Ok(registry)
    }

    pub(crate) fn rebase_portable_record(&self, record: &mut InstallRecord) -> Result<()> {
        if !self.portable {
            return Ok(());
        }
        for (label, value) in [
            ("recipe", record.recipe_id.as_str()),
            ("model", record.model_id.as_str()),
            ("version", record.version.as_str()),
            ("platform", record.platform.as_str()),
        ] {
            if !single_path_component(value) {
                return Err(Error::Other(format!(
                    "portable registry {label} is not a safe path component"
                )));
            }
        }
        let generation = self
            .installs_dir()
            .join(&record.recipe_id)
            .join(&record.model_id)
            .join(&record.version)
            .join(&record.platform);
        if generation == record.generation_root {
            return Ok(());
        }
        let executable = record
            .executable
            .strip_prefix(&record.generation_root)
            .map_err(|_| {
                Error::Other("portable registry executable escaped its generation".into())
            })?
            .to_path_buf();
        let working_directory = record
            .working_directory
            .strip_prefix(&record.generation_root)
            .map_err(|_| {
                Error::Other("portable registry working directory escaped its generation".into())
            })?
            .to_path_buf();
        if !safe_relative_path(&executable, false) || !safe_relative_path(&working_directory, true)
        {
            return Err(Error::Other(
                "portable registry contains a non-portable installed path".into(),
            ));
        }
        record.generation_root.clone_from(&generation);
        record.executable = generation.join(&executable);
        record.working_directory = generation.join(&working_directory);
        Ok(())
    }

    pub fn save(&self, registry: &Registry) -> Result<()> {
        if registry.schema != REGISTRY_SCHEMA {
            return Err(Error::Other(
                "refusing to write invalid registry schema".into(),
            ));
        }
        fs::create_dir_all(&self.data_root).map_err(|source| Error::io(&self.data_root, source))?;
        let path = self.registry_path();
        let next = path.with_extension("next");
        let backup = path.with_extension("backup");
        remove_file_if_present(&next)?;
        let bytes = serde_json::to_vec_pretty(registry)?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&next)
            .map_err(|source| Error::io(&next, source))?;
        file.write_all(&bytes)
            .map_err(|source| Error::io(&next, source))?;
        file.write_all(b"\n")
            .map_err(|source| Error::io(&next, source))?;
        file.sync_all().map_err(|source| Error::io(&next, source))?;

        remove_file_if_present(&backup)?;
        if path.exists() {
            fs::rename(&path, &backup).map_err(|source| Error::io(&path, source))?;
        }
        if let Err(source) = fs::rename(&next, &path) {
            if backup.exists() {
                let _ = fs::rename(&backup, &path);
            }
            return Err(Error::io(&path, source));
        }
        remove_file_if_present(&backup)?;
        sync_directory(&self.data_root)?;
        Ok(())
    }

    pub fn add(&self, record: InstallRecord) -> Result<()> {
        let mut registry = self.load()?;
        registry
            .installs
            .retain(|existing| existing.install_id != record.install_id);
        registry.installs.push(record);
        registry.installs.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.version.cmp(&right.version))
                .then(left.platform.cmp(&right.platform))
        });
        self.save(&registry)
    }

    pub fn forget(&self, install_id: &str) -> Result<bool> {
        let mut registry = self.load()?;
        let original = registry.installs.len();
        registry
            .installs
            .retain(|record| record.install_id != install_id);
        if registry.installs.len() == original {
            return Ok(false);
        }
        self.save(&registry)?;
        Ok(true)
    }

    /// Explicitly removes one immutable generation and its registry record.
    pub fn remove(&self, install_id: &str) -> Result<bool> {
        let registry = self.load()?;
        let Some(record) = registry
            .installs
            .iter()
            .find(|record| record.install_id == install_id)
            .cloned()
        else {
            return Ok(false);
        };
        if !record.generation_root.exists() {
            self.forget(install_id)?;
            return Ok(true);
        }
        let installs = fs::canonicalize(self.installs_dir())
            .map_err(|source| Error::io(self.installs_dir(), source))?;
        let generation = fs::canonicalize(&record.generation_root)
            .map_err(|source| Error::io(&record.generation_root, source))?;
        if !generation.starts_with(&installs) || generation == installs {
            return Err(Error::Other(format!(
                "refusing to remove path outside install generations: {}",
                generation.display()
            )));
        }
        fs::remove_dir_all(&generation).map_err(|source| Error::io(&generation, source))?;
        self.forget(install_id)?;
        Ok(true)
    }

    pub fn integrity(record: &InstallRecord) -> Result<Integrity> {
        if !record.generation_root.is_dir() || !record.executable.is_file() {
            return Ok(Integrity::Missing);
        }
        let generation = fs::canonicalize(&record.generation_root)
            .map_err(|source| Error::io(&record.generation_root, source))?;
        let executable = fs::canonicalize(&record.executable)
            .map_err(|source| Error::io(&record.executable, source))?;
        if !executable.starts_with(&generation) || !executable.is_file() {
            return Ok(Integrity::Changed {
                expected: record.executable_sha256.clone(),
                actual: "executable escaped its generation".into(),
            });
        }
        let executable_actual = sha256_file(&record.executable)?;
        if !executable_actual.eq_ignore_ascii_case(&record.executable_sha256) {
            return Ok(Integrity::Changed {
                expected: record.executable_sha256.clone(),
                actual: executable_actual,
            });
        }
        let snapshot = package_snapshot(&record.generation_root)?;
        if snapshot.sha256.eq_ignore_ascii_case(&record.package_sha256)
            && snapshot.file_count == record.package_file_count
            && snapshot.byte_count == record.package_byte_count
        {
            Ok(Integrity::Verified)
        } else {
            Ok(Integrity::Changed {
                expected: record.package_sha256.clone(),
                actual: snapshot.sha256,
            })
        }
    }
}

fn portable_data_root(executable: &Path) -> Option<PathBuf> {
    let executable_directory = executable.parent()?;
    if executable_directory.join(PORTABLE_MARKER).is_file() {
        return Some(executable_directory.join(PORTABLE_DATA_DIRECTORY));
    }

    // A packaged macOS executable lives in Foo.app/Contents/MacOS while its
    // marker is carried in Contents/Resources. Keep mutable engines beside the
    // .app bundle so the bundle itself remains self-contained and replaceable.
    let contents = executable_directory.parent()?;
    if executable_directory.file_name()?.to_str()? == "MacOS"
        && contents.join("Resources").join(PORTABLE_MARKER).is_file()
    {
        let app = contents.parent()?;
        return app
            .parent()
            .map(|parent| parent.join(PORTABLE_DATA_DIRECTORY));
    }
    None
}

fn single_path_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

fn safe_relative_path(path: &Path, allow_empty: bool) -> bool {
    (allow_empty || !path.as_os_str().is_empty())
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackageSnapshot {
    pub sha256: String,
    pub file_count: u64,
    pub byte_count: u64,
}

pub(crate) fn package_snapshot(root: &Path) -> Result<PackageSnapshot> {
    let root_metadata = fs::symlink_metadata(root).map_err(|source| Error::io(root, source))?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(Error::UnsafeArchiveEntry(root.display().to_string()));
    }
    let mut files = Vec::new();
    let mut seen = BTreeMap::new();
    let mut entry_count = 0_u64;
    let mut byte_count = 0_u64;
    collect_package_files(
        root,
        root,
        &mut files,
        &mut seen,
        &mut entry_count,
        &mut byte_count,
    )?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut package_digest = Sha256::new();
    for (relative, path, length) in &files {
        let file_digest = sha256_file(path)?;
        package_digest.update((relative.len() as u64).to_le_bytes());
        package_digest.update(relative.as_bytes());
        package_digest.update(length.to_le_bytes());
        package_digest.update(file_digest.as_bytes());
    }
    Ok(PackageSnapshot {
        sha256: format!("{:x}", package_digest.finalize()),
        file_count: files.len() as u64,
        byte_count,
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_package_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf, u64)>,
    seen: &mut BTreeMap<String, String>,
    entry_count: &mut u64,
    byte_count: &mut u64,
) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(|source| Error::io(directory, source))? {
        let entry = entry.map_err(|source| Error::io(directory, source))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| Error::UnsafeArchiveEntry(path.display().to_string()))?;
        let portable = portable_relative_path(relative)?;
        let folded = portable.to_lowercase();
        if let Some(previous) = seen.insert(folded, portable.clone()) {
            return Err(Error::UnsafeArchiveEntry(format!(
                "package path collision: `{previous}` and `{portable}`"
            )));
        }
        *entry_count = entry_count.saturating_add(1);
        if *entry_count > MAX_ARCHIVE_ENTRIES + 1 {
            return Err(Error::ArchiveLimit(format!(
                "installed package has more than {} entries",
                MAX_ARCHIVE_ENTRIES + 1
            )));
        }
        let file_type = entry
            .file_type()
            .map_err(|source| Error::io(&path, source))?;
        if file_type.is_dir() {
            collect_package_files(root, &path, files, seen, entry_count, byte_count)?;
        } else if file_type.is_file() {
            if relative == Path::new(INSTALL_RECORD_FILE) {
                continue;
            }
            let length = entry
                .metadata()
                .map_err(|source| Error::io(&path, source))?
                .len();
            if length > MAX_ARCHIVE_ENTRY_BYTES {
                return Err(Error::ArchiveLimit(format!(
                    "installed file `{portable}` exceeds {MAX_ARCHIVE_ENTRY_BYTES} bytes"
                )));
            }
            *byte_count = byte_count.saturating_add(length);
            if *byte_count > MAX_EXTRACTED_BYTES {
                return Err(Error::ArchiveLimit(format!(
                    "installed package exceeds {MAX_EXTRACTED_BYTES} bytes"
                )));
            }
            files.push((portable, path, length));
        } else {
            return Err(Error::UnsafeArchiveEntry(portable));
        }
    }
    Ok(())
}

fn portable_relative_path(path: &Path) -> Result<String> {
    let mut components = Vec::new();
    for component in path.components() {
        let std::path::Component::Normal(value) = component else {
            return Err(Error::UnsafeArchiveEntry(path.display().to_string()));
        };
        let value = value
            .to_str()
            .ok_or_else(|| Error::UnsafeArchiveEntry(path.display().to_string()))?;
        components.push(value);
    }
    if components.is_empty() {
        Err(Error::UnsafeArchiveEntry(path.display().to_string()))
    } else {
        Ok(components.join("/"))
    }
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::io(path, source)),
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    let directory = std::fs::File::open(path).map_err(|source| Error::io(path, source))?;
    directory
        .sync_all()
        .map_err(|source| Error::io(path, source))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_marker_keeps_mutable_data_beside_the_executable() {
        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("uci-grabber.exe");
        std::fs::write(&executable, b"fixture").unwrap();
        std::fs::write(temporary.path().join(PORTABLE_MARKER), b"portable\n").unwrap();
        assert_eq!(
            portable_data_root(&executable),
            Some(temporary.path().join(PORTABLE_DATA_DIRECTORY))
        );
    }

    #[test]
    fn macos_portable_data_stays_outside_the_app_bundle() {
        let temporary = tempfile::tempdir().unwrap();
        let contents = temporary.path().join("UCI Grabber.app/Contents");
        let executable = contents.join("MacOS/uci-grabber");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::create_dir_all(contents.join("Resources")).unwrap();
        std::fs::write(&executable, b"fixture").unwrap();
        std::fs::write(
            contents.join("Resources").join(PORTABLE_MARKER),
            b"portable\n",
        )
        .unwrap();
        assert_eq!(
            portable_data_root(&executable),
            Some(temporary.path().join(PORTABLE_DATA_DIRECTORY))
        );
    }

    #[test]
    fn source_build_without_marker_uses_the_platform_default() {
        let temporary = tempfile::tempdir().unwrap();
        assert_eq!(
            portable_data_root(&temporary.path().join("uci-grabber")),
            None
        );
    }

    #[test]
    fn registry_round_trip_and_forget() {
        let temporary = tempfile::tempdir().unwrap();
        let store = RegistryStore::new(temporary.path());
        let record = InstallRecord {
            install_id: "engine:small:1:linux-x86_64".into(),
            recipe_id: "engine".into(),
            recipe_sha256: "c".repeat(64),
            model_id: "small".into(),
            name: "Engine Small".into(),
            version: "1".into(),
            platform: "linux-x86_64".into(),
            generation_root: temporary.path().join("generation"),
            executable: temporary.path().join("generation/engine"),
            executable_sha256: "a".repeat(64),
            package_sha256: "b".repeat(64),
            package_file_count: 1,
            package_byte_count: 1,
            working_directory: temporary.path().join("generation"),
            source: InstallSource::UnreviewedRecipe,
            installed_at_unix: 1,
            publisher: "Publisher".into(),
            license_spdx: "MIT".into(),
            license_url: "https://example.test/license".into(),
        };
        store.add(record.clone()).unwrap();
        assert_eq!(store.load().unwrap().installs, vec![record]);
        assert!(store.forget("engine:small:1:linux-x86_64").unwrap());
        assert!(store.load().unwrap().installs.is_empty());
    }

    #[test]
    fn explicit_remove_forgets_an_already_missing_generation() {
        let temporary = tempfile::tempdir().unwrap();
        let store = RegistryStore::new(temporary.path());
        let generation = temporary
            .path()
            .join("installs/engine/small/1/linux-x86_64");
        store
            .add(InstallRecord {
                install_id: "engine:small:1:linux-x86_64".into(),
                recipe_id: "engine".into(),
                recipe_sha256: "c".repeat(64),
                model_id: "small".into(),
                name: "Engine Small".into(),
                version: "1".into(),
                platform: "linux-x86_64".into(),
                executable: generation.join("engine"),
                executable_sha256: "a".repeat(64),
                package_sha256: "b".repeat(64),
                package_file_count: 1,
                package_byte_count: 1,
                working_directory: generation.clone(),
                generation_root: generation,
                source: InstallSource::UnreviewedRecipe,
                installed_at_unix: 1,
                publisher: "Publisher".into(),
                license_spdx: "MIT".into(),
                license_url: "https://example.test/license".into(),
            })
            .unwrap();
        assert!(store.remove("engine:small:1:linux-x86_64").unwrap());
        assert!(store.load().unwrap().installs.is_empty());
    }
}
