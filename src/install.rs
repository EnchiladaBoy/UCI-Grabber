use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};

use crate::download::{Downloader, HttpDownloader, sha256_file};
use crate::registry::{
    INSTALL_RECORD_FILE, InstallRecord, InstallSource, Integrity, RegistryStore, package_snapshot,
};
use crate::schema::{ArchiveFormat, Artifact, Package, Recipe, current_platform};
use crate::uci::{ValidationTimeouts, validate_engine_with_cancel};
use crate::{Error, Result, extract};

#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub source: InstallSource,
    pub approve_unreviewed: bool,
    pub platform: Option<String>,
    pub validation_timeouts: ValidationTimeouts,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            source: InstallSource::UnreviewedRecipe,
            approve_unreviewed: false,
            platform: None,
            validation_timeouts: ValidationTimeouts::default(),
        }
    }
}

#[derive(Clone)]
pub struct Installer {
    store: RegistryStore,
    downloader: Arc<dyn Downloader>,
}

impl std::fmt::Debug for Installer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Installer")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

impl Installer {
    pub fn default_store() -> Result<Self> {
        Ok(Self::new(
            RegistryStore::open_default()?,
            Arc::new(HttpDownloader::default()),
        ))
    }

    pub fn new(store: RegistryStore, downloader: Arc<dyn Downloader>) -> Self {
        Self { store, downloader }
    }

    pub fn store(&self) -> &RegistryStore {
        &self.store
    }

    /// Downloads, verifies, validates, and atomically activates one immutable package.
    pub fn install(
        &self,
        recipe: &Recipe,
        model_id: &str,
        options: &InstallOptions,
        cancel: &AtomicBool,
    ) -> Result<InstallRecord> {
        recipe.validate()?;
        if options.source == InstallSource::UnreviewedRecipe && !options.approve_unreviewed {
            return Err(Error::InvalidRecipe(
                "unreviewed recipes require explicit approval: UCI testing runs the downloaded native executable with the user's account permissions and no OS sandbox".into(),
            ));
        }
        let platform = match options.platform.as_deref() {
            Some(platform) => platform,
            None => current_platform()?,
        };
        let model = recipe.model(model_id)?;
        let package = model
            .packages
            .iter()
            .find(|package| package.platform == platform)
            .ok_or_else(|| Error::UnsupportedPlatform(platform.to_owned()))?;
        validate_destination_collisions(package)?;
        let recipe_sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(recipe)?));

        let generation = self
            .store
            .installs_dir()
            .join(&recipe.id)
            .join(&model.id)
            .join(&recipe.version)
            .join(platform);
        let install_id = format!("{}:{}:{}:{}", recipe.id, model.id, recipe.version, platform);
        if generation.exists() {
            let registry = self.store.load()?;
            let record = registry
                .installs
                .into_iter()
                .find(|record| record.install_id == install_id)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "immutable generation already exists but is not registered: {} (run status --repair)",
                        generation.display()
                    ))
                })?;
            if record.source != options.source || record.recipe_sha256 != recipe_sha256 {
                return Err(Error::Other(format!(
                    "immutable-version conflict for {install_id}: the existing generation was created from different recipe bytes or trust source"
                )));
            }
            return match RegistryStore::integrity(&record)? {
                Integrity::Verified => Ok(record),
                Integrity::Missing | Integrity::Changed { .. } => Err(Error::Other(format!(
                    "immutable generation exists but failed integrity verification: {}",
                    generation.display()
                ))),
            };
        }

        let parent = generation
            .parent()
            .ok_or_else(|| Error::Other("invalid generation path".into()))?;
        fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let staging = parent.join(format!(".staging-{}-{nonce}", std::process::id()));
        fs::create_dir(&staging).map_err(|source| Error::io(&staging, source))?;
        let result = self.stage(
            recipe,
            model.id.as_str(),
            &model.name,
            package,
            platform,
            &install_id,
            &generation,
            &staging,
            &recipe_sha256,
            options,
            cancel,
        );
        if result.is_err() && staging.exists() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn stage(
        &self,
        recipe: &Recipe,
        model_id: &str,
        model_name: &str,
        package: &Package,
        platform: &str,
        install_id: &str,
        generation: &Path,
        staging: &Path,
        recipe_sha256: &str,
        options: &InstallOptions,
        cancel: &AtomicBool,
    ) -> Result<InstallRecord> {
        let downloads = staging.join(".downloads");
        fs::create_dir(&downloads).map_err(|source| Error::io(&downloads, source))?;
        let mut extraction_budget = extract::ExtractionBudget::for_generation();
        for (index, artifact) in package.artifacts.iter().enumerate() {
            check_cancel(cancel)?;
            let downloaded = downloads.join(format!("artifact-{index}.part"));
            self.downloader.download(artifact, &downloaded, cancel)?;
            check_cancel(cancel)?;
            independently_verify_download(artifact, &downloaded)?;
            extract::materialize(artifact, &downloaded, staging, &mut extraction_budget)?;
            check_cancel(cancel)?;
            fs::remove_file(&downloaded).map_err(|source| Error::io(&downloaded, source))?;
        }
        fs::remove_dir(&downloads).map_err(|source| Error::io(&downloads, source))?;

        let executable = staging.join(&package.executable);
        let working_directory = executable
            .parent()
            .ok_or_else(|| Error::InvalidRecipe("executable has no parent directory".into()))?
            .to_path_buf();
        validate_staged_paths(staging, &executable, &working_directory)?;
        let snapshot = package_snapshot(staging)?;
        make_executable(&executable)?;
        check_cancel(cancel)?;
        validate_engine_with_cancel(
            &executable,
            &working_directory,
            options.validation_timeouts,
            cancel,
        )?;
        check_cancel(cancel)?;
        if package_snapshot(staging)? != snapshot {
            return Err(Error::Other(
                "staged package changed during UCI validation".into(),
            ));
        }
        let executable_sha256 = sha256_file(&executable)?;
        let relative_executable = executable
            .strip_prefix(staging)
            .map_err(|_| Error::Other("executable escaped staging".into()))?;
        let relative_working_directory = working_directory
            .strip_prefix(staging)
            .map_err(|_| Error::Other("working directory escaped staging".into()))?;
        let record = InstallRecord {
            install_id: install_id.to_owned(),
            recipe_id: recipe.id.clone(),
            recipe_sha256: recipe_sha256.to_owned(),
            model_id: model_id.to_owned(),
            name: format!("{} — {model_name}", recipe.name),
            version: recipe.version.clone(),
            platform: platform.to_owned(),
            generation_root: generation.to_path_buf(),
            executable: generation.join(relative_executable),
            executable_sha256,
            package_sha256: snapshot.sha256.clone(),
            package_file_count: snapshot.file_count,
            package_byte_count: snapshot.byte_count,
            working_directory: generation.join(relative_working_directory),
            source: options.source,
            installed_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            publisher: recipe.publisher.name.clone(),
            license_spdx: recipe.license.spdx.clone(),
            license_url: recipe.license.url.clone(),
        };
        write_record(staging, &record)?;
        if package_snapshot(staging)? != snapshot {
            return Err(Error::Other(
                "staged package changed after UCI validation".into(),
            ));
        }
        sync_tree(staging)?;
        check_cancel(cancel)?;
        fs::rename(staging, generation).map_err(|source| Error::io(generation, source))?;
        sync_directory(
            generation
                .parent()
                .ok_or_else(|| Error::Other("generation has no parent".into()))?,
        )?;
        self.store.add(record.clone())?;
        Ok(record)
    }

    /// Removes interrupted staging directories and registers successfully activated
    /// generations whose final registry write was interrupted.
    pub fn recover(&self) -> Result<RecoveryReport> {
        let installs = self.store.installs_dir();
        if !installs.exists() {
            return Ok(RecoveryReport::default());
        }
        let mut report = RecoveryReport::default();
        let mut records = Vec::new();
        scan_install_tree(&installs, &mut |path| {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if path.is_dir() && name.starts_with(".staging-") {
                fs::remove_dir_all(path).map_err(|source| Error::io(path, source))?;
                report.cleaned_staging += 1;
            } else if path.is_file() && name == INSTALL_RECORD_FILE {
                let bytes = fs::read(path).map_err(|source| Error::io(path, source))?;
                let record: InstallRecord = serde_json::from_slice(&bytes)?;
                validate_recovered_record(&installs, path, &record)?;
                records.push(record);
            }
            Ok(())
        })?;
        let existing = self.store.load()?;
        let existing_ids: BTreeSet<_> = existing
            .installs
            .iter()
            .map(|record| record.install_id.as_str())
            .collect();
        for record in records {
            if !existing_ids.contains(record.install_id.as_str())
                && record.executable.is_file()
                && RegistryStore::integrity(&record)? == Integrity::Verified
            {
                self.store.add(record)?;
                report.repaired_records += 1;
            }
        }
        Ok(report)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub cleaned_staging: usize,
    pub repaired_records: usize,
}

fn independently_verify_download(artifact: &Artifact, path: &Path) -> Result<()> {
    let length = fs::metadata(path)
        .map_err(|source| Error::io(path, source))?
        .len();
    if length != artifact.byte_count {
        return Err(Error::Download {
            url: artifact.url.clone(),
            message: format!(
                "downloader produced {length} bytes, recipe declares {}",
                artifact.byte_count
            ),
        });
    }
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(Error::ChecksumMismatch {
            path: path.to_path_buf(),
            expected: artifact.sha256.clone(),
            actual,
        });
    }
    Ok(())
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn validate_destination_collisions(package: &Package) -> Result<()> {
    let mut seen = BTreeSet::new();
    for artifact in &package.artifacts {
        let folded = artifact.destination.to_lowercase();
        if !seen.insert(folded) {
            return Err(Error::InvalidRecipe(format!(
                "artifact destination collision at `{}`",
                artifact.destination
            )));
        }
        if artifact.format != ArchiveFormat::Raw && artifact.destination == package.executable {
            return Err(Error::InvalidRecipe(
                "archive destination cannot be the executable file".into(),
            ));
        }
    }
    Ok(())
}

fn validate_staged_paths(root: &Path, executable: &Path, working_directory: &Path) -> Result<()> {
    let root = fs::canonicalize(root).map_err(|source| Error::io(root, source))?;
    let executable_metadata =
        fs::symlink_metadata(executable).map_err(|source| Error::io(executable, source))?;
    if !executable_metadata.is_file() || executable_metadata.file_type().is_symlink() {
        return Err(Error::InvalidRecipe(format!(
            "package executable is not a regular file: {}",
            executable.display()
        )));
    }
    let executable_real =
        fs::canonicalize(executable).map_err(|source| Error::io(executable, source))?;
    let working_real = fs::canonicalize(working_directory)
        .map_err(|source| Error::io(working_directory, source))?;
    if !working_real.is_dir()
        || !executable_real.starts_with(&root)
        || !working_real.starts_with(&root)
    {
        return Err(Error::InvalidRecipe(
            "executable or working directory escaped the staged package".into(),
        ));
    }
    Ok(())
}

fn write_record(staging: &Path, record: &InstallRecord) -> Result<()> {
    let path = staging.join(INSTALL_RECORD_FILE);
    let bytes = serde_json::to_vec_pretty(record)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|source| Error::io(&path, source))?;
    file.write_all(&bytes)
        .map_err(|source| Error::io(&path, source))?;
    file.write_all(b"\n")
        .map_err(|source| Error::io(&path, source))?;
    file.sync_all().map_err(|source| Error::io(&path, source))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let metadata = fs::metadata(path).map_err(|source| Error::io(path, source))?;
    let mode = metadata.permissions().mode() | 0o500;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|source| Error::io(path, source))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn sync_tree(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path).map_err(|source| Error::io(path, source))? {
        let entry = entry.map_err(|source| Error::io(path, source))?;
        let child = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| Error::io(&child, source))?;
        if file_type.is_dir() {
            sync_tree(&child)?;
        } else if file_type.is_file() {
            FileSync::sync(&child)?;
        } else {
            return Err(Error::UnsafeArchiveEntry(child.display().to_string()));
        }
    }
    sync_directory(path)
}

struct FileSync;

impl FileSync {
    fn sync(path: &Path) -> Result<()> {
        let file = fs::File::open(path).map_err(|source| Error::io(path, source))?;
        file.sync_all().map_err(|source| Error::io(path, source))
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    FileSync::sync(path)
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn scan_install_tree(path: &Path, visit: &mut impl FnMut(&Path) -> Result<()>) -> Result<()> {
    for entry in fs::read_dir(path).map_err(|source| Error::io(path, source))? {
        let entry = entry.map_err(|source| Error::io(path, source))?;
        let child = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| Error::io(&child, source))?;
        visit(&child)?;
        if file_type.is_dir() && child.exists() {
            scan_install_tree(&child, visit)?;
        }
    }
    Ok(())
}

fn validate_recovered_record(
    installs: &Path,
    manifest_path: &Path,
    record: &InstallRecord,
) -> Result<()> {
    let installs = fs::canonicalize(installs).map_err(|source| Error::io(installs, source))?;
    let manifest_parent = manifest_path
        .parent()
        .ok_or_else(|| Error::Other("recovery manifest has no parent".into()))?;
    let generation =
        fs::canonicalize(manifest_parent).map_err(|source| Error::io(manifest_parent, source))?;
    if generation == installs
        || !generation.starts_with(&installs)
        || record.generation_root != manifest_parent
    {
        return Err(Error::Other(format!(
            "recovery record points outside its install generation: {}",
            manifest_path.display()
        )));
    }
    let executable = fs::canonicalize(&record.executable)
        .map_err(|source| Error::io(&record.executable, source))?;
    let working = fs::canonicalize(&record.working_directory)
        .map_err(|source| Error::io(&record.working_directory, source))?;
    if !executable.is_file()
        || !working.is_dir()
        || !executable.starts_with(&generation)
        || !working.starts_with(&generation)
    {
        return Err(Error::Other(format!(
            "recovery record paths escape generation {}",
            generation.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use sha2::{Digest as _, Sha256};

    use super::*;
    use crate::schema::{ArtifactKind, License, Model, Publisher, RECIPE_SCHEMA};

    struct FixtureDownloader {
        bytes: Vec<u8>,
        calls: Option<Arc<AtomicUsize>>,
    }

    impl Downloader for FixtureDownloader {
        fn download(
            &self,
            _artifact: &Artifact,
            destination: &Path,
            _cancel: &AtomicBool,
        ) -> Result<()> {
            if let Some(calls) = &self.calls {
                calls.fetch_add(1, Ordering::Relaxed);
            }
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(destination)
                .map_err(|source| Error::io(destination, source))?;
            file.write_all(&self.bytes)
                .map_err(|source| Error::io(destination, source))?;
            file.sync_all()
                .map_err(|source| Error::io(destination, source))
        }
    }

    #[cfg(unix)]
    #[test]
    fn installs_and_registers_fake_engine_immutably() {
        let script = b"#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\nuci) printf 'id name Installed Fixture\\nuciok\\n';;\nisready) printf 'readyok\\n';;\n'go depth 1') printf 'bestmove e2e4\\n';;\nquit) exit 0;;\nesac\ndone\n".to_vec();
        let digest = format!("{:x}", Sha256::digest(&script));
        let platform = current_platform().unwrap().to_owned();
        let recipe = Recipe {
            schema: RECIPE_SCHEMA.into(),
            id: "fixture-engine".into(),
            name: "Fixture Engine".into(),
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
            homepage: "https://example.test".into(),
            minimum_fisheye_version: "1.7.0".into(),
            models: vec![Model {
                id: "small".into(),
                name: "Small".into(),
                description: "Fixture".into(),
                packages: vec![Package {
                    platform: platform.clone(),
                    artifacts: vec![Artifact {
                        kind: ArtifactKind::Runtime,
                        url: "https://example.test/engine".into(),
                        byte_count: script.len() as u64,
                        sha256: digest,
                        format: ArchiveFormat::Raw,
                        destination: "engine".into(),
                    }],
                    executable: "engine".into(),
                    working_directory: ".".into(),
                }],
            }],
        };
        let temporary = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let installer = Installer::new(
            RegistryStore::new(temporary.path()),
            Arc::new(FixtureDownloader {
                bytes: script,
                calls: Some(Arc::clone(&calls)),
            }),
        );
        let options = InstallOptions {
            approve_unreviewed: true,
            platform: Some(platform),
            ..InstallOptions::default()
        };
        let record = installer
            .install(&recipe, "small", &options, &AtomicBool::new(false))
            .unwrap();
        assert!(record.executable.is_file());
        assert_eq!(installer.store.load().unwrap().installs.len(), 1);
        let again = installer
            .install(&recipe, "small", &options, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(again.install_id, record.install_id);
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        let mut changed_recipe = recipe.clone();
        changed_recipe.description = "Changed recipe bytes under the same version".into();
        let error = installer
            .install(&changed_recipe, "small", &options, &AtomicBool::new(false))
            .unwrap_err();
        assert!(error.to_string().contains("immutable-version conflict"));

        let mut curated_options = options.clone();
        curated_options.source = InstallSource::Curated;
        let error = installer
            .install(&recipe, "small", &curated_options, &AtomicBool::new(false))
            .unwrap_err();
        assert!(error.to_string().contains("immutable-version conflict"));
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        std::fs::write(
            record.generation_root.join("unexpected-model.bin"),
            b"changed",
        )
        .unwrap();
        assert!(matches!(
            RegistryStore::integrity(&record).unwrap(),
            Integrity::Changed { .. }
        ));
    }

    #[test]
    fn unreviewed_recipe_requires_explicit_approval() {
        let temporary = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let installer = Installer::new(
            RegistryStore::new(temporary.path()),
            Arc::new(FixtureDownloader {
                bytes: Vec::new(),
                calls: Some(Arc::clone(&calls)),
            }),
        );
        let recipe = Recipe::from_json(include_bytes!(
            "../catalog/tests/fixtures/valid-recipe.json"
        ))
        .unwrap();
        let error = installer
            .install(
                &recipe,
                "standard",
                &InstallOptions::default(),
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert!(error.to_string().contains("explicit approval"));
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }
}
