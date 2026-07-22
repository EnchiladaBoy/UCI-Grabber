use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Seek as _, Write as _};
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};

use crate::download::{Downloader, HttpDownloader, sha256_file};
use crate::registry::{
    INSTALL_RECORD_FILE, InstallRecord, InstallSource, Integrity, PAYLOAD_REVIEW_COUNT_LEN,
    PAYLOAD_REVIEW_DIGEST_LEN, PAYLOAD_REVIEW_MARKER_LEN, PAYLOAD_REVIEW_PLACEHOLDER,
    RegistryStore, package_payload_snapshot, package_snapshot,
};
use crate::schema::{ArchiveFormat, Artifact, ArtifactKind, Package, Recipe, current_platform};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallPhase {
    Downloading,
    Verifying,
    Extracting,
    ValidatingUci,
    Activating,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstallProgress {
    pub phase: InstallPhase,
    pub artifact_kind: Option<ArtifactKind>,
    pub completed_bytes: u64,
    pub total_bytes: u64,
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
        self.install_with_progress(recipe, model_id, options, cancel, &|_| {})
    }

    /// Downloads, verifies, validates, and atomically activates one immutable
    /// package while reporting progress to interactive frontends.
    pub fn install_with_progress(
        &self,
        recipe: &Recipe,
        model_id: &str,
        options: &InstallOptions,
        cancel: &AtomicBool,
        progress: &(dyn Fn(InstallProgress) + Send + Sync),
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
        let total_bytes = package.artifacts.iter().fold(0_u64, |total, artifact| {
            total.saturating_add(artifact.byte_count)
        });
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
                Integrity::Verified => {
                    progress(InstallProgress {
                        phase: InstallPhase::Ready,
                        artifact_kind: None,
                        completed_bytes: total_bytes,
                        total_bytes,
                    });
                    Ok(record)
                }
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
            total_bytes,
            progress,
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
        total_bytes: u64,
        progress: &(dyn Fn(InstallProgress) + Send + Sync),
    ) -> Result<InstallRecord> {
        let downloads = staging.join(".downloads");
        fs::create_dir(&downloads).map_err(|source| Error::io(&downloads, source))?;
        let mut extraction_budget = extract::ExtractionBudget::for_generation(staging)?;
        let mut completed_bytes = 0_u64;
        for (index, artifact) in package.artifacts.iter().enumerate() {
            check_cancel(cancel)?;
            let downloaded = downloads.join(format!("artifact-{index}.part"));
            report_progress(
                progress,
                InstallPhase::Downloading,
                Some(artifact.kind),
                completed_bytes,
                total_bytes,
            );
            let completed_before_artifact = completed_bytes;
            self.downloader.download_with_progress(
                artifact,
                &downloaded,
                cancel,
                &|downloaded_bytes, _artifact_bytes| {
                    report_progress(
                        progress,
                        InstallPhase::Downloading,
                        Some(artifact.kind),
                        completed_before_artifact.saturating_add(downloaded_bytes),
                        total_bytes,
                    );
                },
            )?;
            completed_bytes = completed_bytes.saturating_add(artifact.byte_count);
            check_cancel(cancel)?;
            report_progress(
                progress,
                InstallPhase::Verifying,
                Some(artifact.kind),
                completed_bytes,
                total_bytes,
            );
            independently_verify_download(artifact, &downloaded)?;
            report_progress(
                progress,
                InstallPhase::Extracting,
                Some(artifact.kind),
                completed_bytes,
                total_bytes,
            );
            extract::materialize(artifact, &downloaded, staging, &mut extraction_budget).map_err(
                |error| {
                    Error::Other(format!(
                        "could not materialize artifact {} at {}: {error}",
                        index, artifact.destination
                    ))
                },
            )?;
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
        make_executable(&executable)?;
        if options.source == InstallSource::Curated && recipe.id == "maia3" {
            personalize_maia_launcher(staging, &executable)?;
        }
        let snapshot = package_snapshot(staging)?;
        check_cancel(cancel)?;
        report_progress(
            progress,
            InstallPhase::ValidatingUci,
            None,
            total_bytes,
            total_bytes,
        );
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
        report_progress(
            progress,
            InstallPhase::Activating,
            None,
            total_bytes,
            total_bytes,
        );
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
        report_progress(
            progress,
            InstallPhase::Ready,
            None,
            total_bytes,
            total_bytes,
        );
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
                let mut record: InstallRecord = serde_json::from_slice(&bytes)?;
                self.store.rebase_portable_record(&mut record)?;
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

fn personalize_maia_launcher(staging: &Path, executable: &Path) -> Result<()> {
    let package_root = staging.join("package");
    let payload = package_payload_snapshot(&package_root, executable)?;
    let mut review = *PAYLOAD_REVIEW_PLACEHOLDER;
    let digest_end = PAYLOAD_REVIEW_MARKER_LEN + PAYLOAD_REVIEW_DIGEST_LEN;
    let files_end = digest_end + PAYLOAD_REVIEW_COUNT_LEN;
    review[PAYLOAD_REVIEW_MARKER_LEN..digest_end].copy_from_slice(payload.sha256.as_bytes());
    let file_count = format!(
        "{:0width$}",
        payload.file_count,
        width = PAYLOAD_REVIEW_COUNT_LEN
    );
    let byte_count = format!(
        "{:0width$}",
        payload.byte_count,
        width = PAYLOAD_REVIEW_COUNT_LEN
    );
    if file_count.len() != PAYLOAD_REVIEW_COUNT_LEN || byte_count.len() != PAYLOAD_REVIEW_COUNT_LEN
    {
        return Err(Error::Other(
            "Maia3 package counts exceed the launcher review fields".into(),
        ));
    }
    review[digest_end..files_end].copy_from_slice(file_count.as_bytes());
    review[files_end..].copy_from_slice(byte_count.as_bytes());

    let bytes = fs::read(executable).map_err(|source| Error::io(executable, source))?;
    let matches: Vec<_> = bytes
        .windows(PAYLOAD_REVIEW_PLACEHOLDER.len())
        .enumerate()
        .filter_map(|(offset, value)| (value == PAYLOAD_REVIEW_PLACEHOLDER).then_some(offset))
        .collect();
    let [offset] = matches.as_slice() else {
        return Err(Error::Other(format!(
            "reviewed Maia3 launcher must contain exactly one unpersonalized payload field; found {}",
            matches.len()
        )));
    };
    let mut launcher = OpenOptions::new()
        .write(true)
        .open(executable)
        .map_err(|source| Error::io(executable, source))?;
    launcher
        .seek(std::io::SeekFrom::Start(*offset as u64))
        .map_err(|source| Error::io(executable, source))?;
    launcher
        .write_all(&review)
        .map_err(|source| Error::io(executable, source))?;
    launcher
        .sync_all()
        .map_err(|source| Error::io(executable, source))?;

    #[cfg(target_os = "macos")]
    {
        let status = Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-", "--timestamp=none"])
            .arg(executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|source| {
                Error::Other(format!("could not ad-hoc sign Maia3 launcher: {source}"))
            })?;
        if !status.success() {
            return Err(Error::Other(format!(
                "could not apply the required local ad-hoc signature to {}",
                executable.display()
            )));
        }
    }
    Ok(())
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

fn report_progress(
    callback: &(dyn Fn(InstallProgress) + Send + Sync),
    phase: InstallPhase,
    artifact_kind: Option<ArtifactKind>,
    completed_bytes: u64,
    total_bytes: u64,
) {
    callback(InstallProgress {
        phase,
        artifact_kind,
        completed_bytes: completed_bytes.min(total_bytes),
        total_bytes,
    });
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
    #[cfg(windows)]
    fn sync(path: &Path) -> Result<()> {
        // File::open requests only read access. Windows' FlushFileBuffers,
        // which backs File::sync_all, requires a handle opened for writing.
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|source| Error::io(path, source))?;
        file.sync_all().map_err(|source| Error::io(path, source))
    }

    #[cfg(not(windows))]
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
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use sha2::{Digest as _, Sha256};

    use super::*;
    use crate::schema::{ArtifactKind, Catalog, License, Model, Publisher, RECIPE_SCHEMA};

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

    struct ReviewedDirectoryDownloader {
        root: std::path::PathBuf,
    }

    impl Downloader for ReviewedDirectoryDownloader {
        fn download(
            &self,
            artifact: &Artifact,
            destination: &Path,
            _cancel: &AtomicBool,
        ) -> Result<()> {
            let artifact_name = match artifact.destination.as_str() {
                "package/launcher" => "launcher",
                "package/python-runtime" => "python-runtime",
                "package/maia-source" => "maia3",
                "package/chess-source" => "chess",
                value if value.starts_with("package/packages/") => value
                    .rsplit('/')
                    .next()
                    .ok_or_else(|| Error::Other("package artifact has no name".into()))?,
                value if value.starts_with("package/launcher/models/") => value
                    .rsplit('/')
                    .next()
                    .ok_or_else(|| Error::Other("model artifact has no name".into()))?,
                value => {
                    return Err(Error::Other(format!(
                        "unexpected reviewed artifact destination: {value}"
                    )));
                }
            };
            let source = self.root.join(artifact_name);
            fs::copy(&source, destination)
                .map(|_| ())
                .map_err(|error| Error::io(&source, error))
        }
    }

    #[cfg(unix)]
    fn fixture_script() -> Vec<u8> {
        b"#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\nuci) printf 'id name Installed Fixture\\nuciok\\n';;\nisready) printf 'readyok\\n';;\n'go depth 1') printf 'bestmove e2e4\\n';;\nquit) exit 0;;\nesac\ndone\n".to_vec()
    }

    #[cfg(unix)]
    fn fixture_recipe(script: &[u8], platform: &str) -> Recipe {
        Recipe {
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
                    platform: platform.into(),
                    artifacts: vec![Artifact {
                        kind: ArtifactKind::Runtime,
                        url: "https://example.test/engine".into(),
                        byte_count: script.len() as u64,
                        sha256: format!("{:x}", Sha256::digest(script)),
                        format: ArchiveFormat::Raw,
                        destination: "engine".into(),
                    }],
                    executable: "engine".into(),
                    working_directory: ".".into(),
                }],
            }],
        }
    }

    #[test]
    fn sync_tree_flushes_dotfiles() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("chess-source/chess-1.11.2");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(".editorconfig"), b"root = true\n").unwrap();

        sync_tree(temporary.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn installs_and_registers_fake_engine_immutably() {
        let script = fixture_script();
        let platform = current_platform().unwrap().to_owned();
        let recipe = fixture_recipe(&script, &platform);
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
        let updates = Mutex::new(Vec::new());
        let record = installer
            .install_with_progress(
                &recipe,
                "small",
                &options,
                &AtomicBool::new(false),
                &|progress| updates.lock().unwrap().push(progress),
            )
            .unwrap();
        assert!(record.executable.is_file());
        let phases: Vec<_> = updates
            .lock()
            .unwrap()
            .iter()
            .map(|progress| progress.phase)
            .collect();
        for expected in [
            InstallPhase::Downloading,
            InstallPhase::Verifying,
            InstallPhase::Extracting,
            InstallPhase::ValidatingUci,
            InstallPhase::Activating,
            InstallPhase::Ready,
        ] {
            assert!(
                phases.contains(&expected),
                "missing progress phase {expected:?}"
            );
        }
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

    #[cfg(unix)]
    #[test]
    fn moving_the_portable_tree_rebases_registry_and_recovery_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let original_bundle = temporary.path().join("original");
        let original_data = original_bundle.join("UCI-Grabber-Data");
        let script = fixture_script();
        let platform = current_platform().unwrap().to_owned();
        let recipe = fixture_recipe(&script, &platform);
        let installer = Installer::new(
            RegistryStore::new_portable(&original_data),
            Arc::new(FixtureDownloader {
                bytes: script,
                calls: None,
            }),
        );
        installer
            .install(
                &recipe,
                "small",
                &InstallOptions {
                    approve_unreviewed: true,
                    platform: Some(platform),
                    ..InstallOptions::default()
                },
                &AtomicBool::new(false),
            )
            .unwrap();

        let moved_bundle = temporary.path().join("moved");
        std::fs::rename(&original_bundle, &moved_bundle).unwrap();
        let moved_store = RegistryStore::new_portable(moved_bundle.join("UCI-Grabber-Data"));
        let moved_installer = Installer::new(
            moved_store.clone(),
            Arc::new(FixtureDownloader {
                bytes: Vec::new(),
                calls: None,
            }),
        );
        moved_installer.recover().unwrap();
        let records = moved_store.load().unwrap().installs;
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert!(record.generation_root.starts_with(&moved_bundle));
        assert_eq!(record.working_directory, record.generation_root);
        assert!(record.executable.is_file());
        assert_eq!(
            RegistryStore::integrity(record).unwrap(),
            Integrity::Verified
        );
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

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn personalizes_maia_launcher_with_the_exact_payload_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let package = temporary.path().join("package");
        let launcher_dir = package.join("launcher");
        std::fs::create_dir_all(&launcher_dir).unwrap();
        let launcher = launcher_dir.join("maia3-launcher");
        let mut launcher_bytes = b"reviewed launcher prefix".to_vec();
        launcher_bytes.extend_from_slice(PAYLOAD_REVIEW_PLACEHOLDER);
        launcher_bytes.extend_from_slice(b"reviewed launcher suffix");
        std::fs::write(&launcher, launcher_bytes).unwrap();
        std::fs::write(package.join("runtime-file"), b"verified payload").unwrap();
        let expected = package_payload_snapshot(&package, &launcher).unwrap();

        personalize_maia_launcher(temporary.path(), &launcher).unwrap();

        let bytes = std::fs::read(&launcher).unwrap();
        assert!(
            !bytes
                .windows(PAYLOAD_REVIEW_PLACEHOLDER.len())
                .any(|value| value == PAYLOAD_REVIEW_PLACEHOLDER)
        );
        let marker = &PAYLOAD_REVIEW_PLACEHOLDER[..PAYLOAD_REVIEW_MARKER_LEN];
        let offset = bytes
            .windows(marker.len())
            .position(|value| value == marker)
            .unwrap();
        let digest_start = offset + PAYLOAD_REVIEW_MARKER_LEN;
        let digest_end = digest_start + PAYLOAD_REVIEW_DIGEST_LEN;
        let files_end = digest_end + PAYLOAD_REVIEW_COUNT_LEN;
        assert_eq!(&bytes[digest_start..digest_end], expected.sha256.as_bytes());
        assert_eq!(
            std::str::from_utf8(&bytes[digest_end..files_end])
                .unwrap()
                .parse::<u64>()
                .unwrap(),
            expected.file_count
        );
        assert_eq!(
            std::str::from_utf8(&bytes[files_end..files_end + PAYLOAD_REVIEW_COUNT_LEN])
                .unwrap()
                .parse::<u64>()
                .unwrap(),
            expected.byte_count
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "set the UCI_GRABBER_REAL_MAIA_* variables to reviewed local artifacts"]
    fn installs_and_validates_the_real_direct_maia_package() {
        let catalog_path = std::env::var_os("UCI_GRABBER_REAL_MAIA_CATALOG")
            .map(std::path::PathBuf::from)
            .expect("UCI_GRABBER_REAL_MAIA_CATALOG is required");
        let artifacts = std::env::var_os("UCI_GRABBER_REAL_MAIA_ARTIFACTS")
            .map(std::path::PathBuf::from)
            .expect("UCI_GRABBER_REAL_MAIA_ARTIFACTS is required");
        let output = std::env::var_os("UCI_GRABBER_REAL_MAIA_OUTPUT")
            .map(std::path::PathBuf::from)
            .expect("UCI_GRABBER_REAL_MAIA_OUTPUT is required");
        assert!(!output.exists(), "real Maia output must not already exist");
        let catalog = Catalog::from_json(&fs::read(&catalog_path).unwrap()).unwrap();
        let recipe = catalog
            .recipes
            .iter()
            .find(|recipe| recipe.id == "maia3")
            .expect("review catalog has no Maia3 recipe");
        let platform = current_platform().unwrap().to_owned();
        let installer = Installer::new(
            RegistryStore::new(&output),
            Arc::new(ReviewedDirectoryDownloader { root: artifacts }),
        );
        let record = installer
            .install(
                recipe,
                "maia3-5m",
                &InstallOptions {
                    source: InstallSource::Curated,
                    platform: Some(platform),
                    ..InstallOptions::default()
                },
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(
            RegistryStore::integrity(&record).unwrap(),
            Integrity::Verified
        );
        println!("real Maia executable: {}", record.executable.display());
    }
}
