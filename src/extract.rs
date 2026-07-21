use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write as _};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;

use crate::schema::{ArchiveFormat, Artifact, is_portable_relative_path};
use crate::{Error, Result};

pub const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_ARCHIVE_ENTRIES: u64 = 40_000;
pub const MAX_ARCHIVE_ENTRY_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_PAX_GLOBAL_HEADER_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub(crate) struct ExtractionBudget {
    byte_limit: u64,
    bytes_used: u64,
    entry_limit: u64,
    entries_used: u64,
    case_insensitive: bool,
    entries: BTreeMap<String, (String, bool)>,
}

impl ExtractionBudget {
    pub(crate) fn for_generation(root: &Path) -> Result<Self> {
        Ok(Self {
            byte_limit: MAX_EXTRACTED_BYTES,
            bytes_used: 0,
            entry_limit: MAX_ARCHIVE_ENTRIES,
            entries_used: 0,
            case_insensitive: filesystem_is_case_insensitive(root)?,
            entries: BTreeMap::new(),
        })
    }

    fn reserve_path(
        &mut self,
        root: &Path,
        path: &Path,
        is_directory: bool,
        bytes: u64,
    ) -> Result<()> {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| Error::UnsafeArchiveEntry(path.display().to_string()))?;
        let components: Vec<_> = relative.components().collect();
        if components.is_empty() {
            return Err(Error::UnsafeArchiveEntry(path.display().to_string()));
        }

        let mut nodes = Vec::with_capacity(components.len());
        let mut prefix = PathBuf::new();
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(value) = component else {
                return Err(Error::UnsafeArchiveEntry(path.display().to_string()));
            };
            prefix.push(value);
            let original = portable_path(&prefix, false);
            let folded = portable_path(&prefix, self.case_insensitive);
            let final_component = index + 1 == components.len();
            let desired_directory = !final_component || is_directory;
            if let Some((known_original, known_directory)) = self.entries.get(&folded) {
                let incompatible = known_original != &original
                    || (!final_component && !known_directory)
                    || (final_component && if is_directory { !known_directory } else { true });
                if incompatible {
                    return Err(Error::UnsafeArchiveEntry(format!(
                        "package path collision: `{known_original}` and `{original}`"
                    )));
                }
            }
            nodes.push((folded, original, desired_directory));
        }

        let additions = u64::try_from(
            nodes
                .iter()
                .filter(|(folded, _, _)| !self.entries.contains_key(folded))
                .count(),
        )
        .map_err(|_| self.entry_limit_error(path))?;
        let Some(next_entries) = self.entries_used.checked_add(additions) else {
            return Err(self.entry_limit_error(path));
        };
        if next_entries > self.entry_limit {
            return Err(self.entry_limit_error(path));
        }
        let Some(next_bytes) = self.bytes_used.checked_add(bytes) else {
            return Err(self.byte_limit_error(path));
        };
        if next_bytes > self.byte_limit {
            return Err(self.byte_limit_error(path));
        }

        for (folded, original, is_directory) in nodes {
            self.entries
                .entry(folded)
                .or_insert((original, is_directory));
        }
        self.entries_used = next_entries;
        self.bytes_used = next_bytes;
        Ok(())
    }

    fn byte_limit_error(&self, path: &Path) -> Error {
        Error::ArchiveLimit(format!(
            "staged package would exceed {} bytes while materializing `{}`",
            self.byte_limit,
            path.display()
        ))
    }

    fn entry_limit_error(&self, path: &Path) -> Error {
        Error::ArchiveLimit(format!(
            "staged package would exceed {} entries while materializing `{}`",
            self.entry_limit,
            path.display()
        ))
    }

    #[cfg(test)]
    fn with_limits(byte_limit: u64, entry_limit: u64, case_insensitive: bool) -> Self {
        Self {
            byte_limit,
            bytes_used: 0,
            entry_limit,
            entries_used: 0,
            case_insensitive,
            entries: BTreeMap::new(),
        }
    }
}

pub(crate) fn filesystem_is_case_insensitive(root: &Path) -> Result<bool> {
    let probe = tempfile::Builder::new()
        .prefix(".uci-grabber-case-probe-")
        .tempdir_in(root)
        .map_err(|source| Error::io(root, source))?;
    let lowercase = probe.path().join("case-sensitive-probe");
    let uppercase = probe.path().join("CASE-SENSITIVE-PROBE");
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&lowercase)
        .map_err(|source| Error::io(&lowercase, source))?;
    let case_insensitive = match fs::symlink_metadata(&uppercase) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(Error::io(&uppercase, error)),
    };
    probe.close().map_err(|source| Error::io(root, source))?;
    Ok(case_insensitive)
}

pub(crate) fn materialize(
    artifact: &Artifact,
    downloaded: &Path,
    root: &Path,
    budget: &mut ExtractionBudget,
) -> Result<()> {
    let destination = checked_join(root, &artifact.destination)?;
    match artifact.format {
        ArchiveFormat::Raw => copy_raw(downloaded, root, &destination, budget),
        ArchiveFormat::Zip => extract_zip(downloaded, root, &destination, budget),
        ArchiveFormat::TarGz => {
            let file = File::open(downloaded).map_err(|source| Error::io(downloaded, source))?;
            extract_tar(GzDecoder::new(file), root, &destination, budget)
        }
        ArchiveFormat::TarZst => {
            let file = File::open(downloaded).map_err(|source| Error::io(downloaded, source))?;
            let decoder = zstd::stream::read::Decoder::new(file)
                .map_err(|source| Error::Other(format!("invalid tar.zst archive: {source}")))?;
            extract_tar(decoder, root, &destination, budget)
        }
    }
}

fn copy_raw(
    source: &Path,
    root: &Path,
    destination: &Path,
    budget: &mut ExtractionBudget,
) -> Result<()> {
    let length = fs::metadata(source)
        .map_err(|error| Error::io(source, error))?
        .len();
    if length > MAX_ARCHIVE_ENTRY_BYTES {
        return Err(Error::ArchiveLimit(format!(
            "raw artifact `{}` exceeds {MAX_ARCHIVE_ENTRY_BYTES} bytes",
            destination.display()
        )));
    }
    budget.reserve_path(root, destination, false, length)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
    }
    let mut input = File::open(source).map_err(|error| Error::io(source, error))?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| Error::io(destination, error))?;
    io::copy(&mut input, &mut output).map_err(|error| Error::io(destination, error))?;
    output
        .sync_all()
        .map_err(|error| Error::io(destination, error))
}

fn extract_zip(
    archive_path: &Path,
    root: &Path,
    destination: &Path,
    budget: &mut ExtractionBudget,
) -> Result<()> {
    budget.reserve_path(root, destination, true, 0)?;
    fs::create_dir_all(destination).map_err(|source| Error::io(destination, source))?;
    let file = File::open(archive_path).map_err(|source| Error::io(archive_path, source))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|source| Error::Other(format!("invalid ZIP archive: {source}")))?;
    if archive.len() as u64 > MAX_ARCHIVE_ENTRIES {
        return Err(Error::ArchiveLimit(format!(
            "more than {MAX_ARCHIVE_ENTRIES} entries"
        )));
    }
    let mut paths = CollisionGuard::new(budget.case_insensitive);
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|source| Error::Other(format!("could not read ZIP entry: {source}")))?;
        let name = entry.name().to_owned();
        let relative = safe_archive_path(Path::new(&name))?;
        let output_path = destination.join(&relative);
        let unix_mode = entry.unix_mode().unwrap_or(0);
        if unix_mode & 0o170_000 == 0o120_000 {
            return Err(Error::UnsafeArchiveEntry(name));
        }
        if entry.is_dir() {
            paths.insert(Path::new(&name), true)?;
            budget.reserve_path(root, &output_path, true, 0)?;
            fs::create_dir_all(&output_path).map_err(|source| Error::io(&output_path, source))?;
            continue;
        }
        if !entry.is_file() || entry.size() > MAX_ARCHIVE_ENTRY_BYTES {
            return Err(Error::ArchiveLimit(format!(
                "unsafe or oversized entry `{name}`"
            )));
        }
        paths.insert(Path::new(&name), false)?;
        budget.reserve_path(root, &output_path, false, entry.size())?;
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
        }
        let output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&output_path)
            .map_err(|source| Error::io(&output_path, source))?;
        let size = entry.size();
        copy_bounded(&mut entry, output, size, &output_path)?;
        set_mode(&output_path, unix_mode)?;
    }
    Ok(())
}

fn extract_tar(
    reader: impl Read,
    root: &Path,
    destination: &Path,
    budget: &mut ExtractionBudget,
) -> Result<()> {
    budget.reserve_path(root, destination, true, 0)?;
    fs::create_dir_all(destination).map_err(|source| Error::io(destination, source))?;
    let mut archive = tar::Archive::new(reader);
    let mut count = 0_u64;
    let mut paths = CollisionGuard::new(budget.case_insensitive);
    let mut regular_files = BTreeMap::new();
    let mut links = BTreeMap::new();
    let mut directories = BTreeSet::new();
    let mut pending_links = Vec::new();
    let entries = archive
        .entries()
        .map_err(|source| Error::Other(format!("invalid tar archive: {source}")))?;
    for entry in entries {
        count += 1;
        if count > MAX_ARCHIVE_ENTRIES {
            return Err(Error::ArchiveLimit(format!(
                "more than {MAX_ARCHIVE_ENTRIES} entries"
            )));
        }
        let mut entry =
            entry.map_err(|source| Error::Other(format!("could not read tar entry: {source}")))?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_pax_global_extensions() {
            validate_pax_global_header(&mut entry)?;
            continue;
        }
        let path = entry
            .path()
            .map_err(|source| Error::Other(format!("invalid tar path: {source}")))?;
        let relative = safe_archive_path(&path)?;
        let output_path = destination.join(&relative);
        let archive_name = portable_path(&relative, false);
        record_parent_directories(&relative, &mut directories);
        if entry_type.is_dir() {
            paths.insert(&path, true)?;
            directories.insert(archive_name);
            budget.reserve_path(root, &output_path, true, 0)?;
            fs::create_dir_all(&output_path).map_err(|source| Error::io(&output_path, source))?;
            continue;
        }
        if entry_type.is_symlink() {
            if entry.size() != 0 {
                return Err(Error::UnsafeArchiveEntry(format!(
                    "tar file link has a nonzero body: `{}`",
                    path.display()
                )));
            }
            paths.insert(&path, false)?;
            let target = entry
                .link_name()
                .map_err(|source| Error::Other(format!("invalid tar link target: {source}")))?
                .ok_or_else(|| Error::UnsafeArchiveEntry(path.display().to_string()))?;
            let target = safe_tar_link_target(&relative, &target)?;
            links.insert(archive_name.clone(), portable_path(&target, false));
            pending_links.push(PendingTarLink {
                archive_name,
                output_path,
            });
            continue;
        }
        if !entry_type.is_file() {
            return Err(Error::UnsafeArchiveEntry(path.display().to_string()));
        }
        paths.insert(&path, false)?;
        let size = entry.size();
        if size > MAX_ARCHIVE_ENTRY_BYTES {
            return Err(Error::ArchiveLimit(format!(
                "entry `{}` exceeds {MAX_ARCHIVE_ENTRY_BYTES} bytes",
                path.display()
            )));
        }
        budget.reserve_path(root, &output_path, false, size)?;
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
        }
        let output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&output_path)
            .map_err(|source| Error::io(&output_path, source))?;
        copy_bounded(&mut entry, output, size, &output_path)?;
        let mode = entry.header().mode().unwrap_or(0);
        set_mode(&output_path, mode)?;
        regular_files.insert(
            archive_name,
            ExtractedTarFile {
                output_path,
                size,
                mode,
            },
        );
    }

    for pending in pending_links {
        let target = resolve_tar_link(&pending.archive_name, &links, &regular_files, &directories)?;
        budget.reserve_path(root, &pending.output_path, false, target.size)?;
        if let Some(parent) = pending.output_path.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
        }
        let mut input = File::open(&target.output_path)
            .map_err(|source| Error::io(&target.output_path, source))?;
        let output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&pending.output_path)
            .map_err(|source| Error::io(&pending.output_path, source))?;
        copy_bounded(&mut input, output, target.size, &pending.output_path)?;
        set_mode(&pending.output_path, target.mode)?;
    }
    Ok(())
}

fn validate_pax_global_header(entry: &mut tar::Entry<'_, impl Read>) -> Result<()> {
    if entry.size() > MAX_PAX_GLOBAL_HEADER_BYTES {
        return Err(Error::ArchiveLimit(format!(
            "global PAX header exceeds {MAX_PAX_GLOBAL_HEADER_BYTES} bytes"
        )));
    }
    let extensions = entry
        .pax_extensions()
        .map_err(|source| Error::Other(format!("invalid global PAX header: {source}")))?
        .ok_or_else(|| Error::UnsafeArchiveEntry("missing global PAX metadata".into()))?;
    for extension in extensions {
        let extension = extension
            .map_err(|source| Error::Other(format!("invalid global PAX record: {source}")))?;
        if extension.key_bytes() != b"comment" {
            return Err(Error::UnsafeArchiveEntry(format!(
                "unsupported global PAX key `{}`",
                String::from_utf8_lossy(extension.key_bytes())
            )));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ExtractedTarFile {
    output_path: PathBuf,
    size: u64,
    mode: u32,
}

#[derive(Debug)]
struct PendingTarLink {
    archive_name: String,
    output_path: PathBuf,
}

fn record_parent_directories(path: &Path, directories: &mut BTreeSet<String>) {
    let mut parent = PathBuf::new();
    let components: Vec<_> = path.components().collect();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        if let Component::Normal(value) = component {
            parent.push(value);
            directories.insert(portable_path(&parent, false));
        }
    }
}

fn safe_tar_link_target(link_path: &Path, target: &Path) -> Result<PathBuf> {
    let display = target
        .to_str()
        .ok_or_else(|| Error::UnsafeArchiveEntry(target.display().to_string()))?;
    if display.is_empty() || display.starts_with('/') || display.ends_with('/') {
        return Err(Error::UnsafeArchiveEntry(format!(
            "{} -> {}",
            link_path.display(),
            target.display()
        )));
    }

    let mut result = link_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .to_path_buf();
    for component in display.split('/') {
        match component {
            "" => {
                return Err(Error::UnsafeArchiveEntry(format!(
                    "{} -> {}",
                    link_path.display(),
                    target.display()
                )));
            }
            "." => {}
            ".." => {
                if !result.pop() {
                    return Err(Error::UnsafeArchiveEntry(format!(
                        "{} -> {}",
                        link_path.display(),
                        target.display()
                    )));
                }
            }
            value if is_portable_relative_path(value, false) => result.push(value),
            _ => {
                return Err(Error::UnsafeArchiveEntry(format!(
                    "{} -> {}",
                    link_path.display(),
                    target.display()
                )));
            }
        }
    }
    if result.as_os_str().is_empty() {
        return Err(Error::UnsafeArchiveEntry(format!(
            "{} -> {}",
            link_path.display(),
            target.display()
        )));
    }
    Ok(result)
}

fn resolve_tar_link<'a>(
    link: &str,
    links: &BTreeMap<String, String>,
    regular_files: &'a BTreeMap<String, ExtractedTarFile>,
    directories: &BTreeSet<String>,
) -> Result<&'a ExtractedTarFile> {
    let mut visited = BTreeSet::new();
    let mut current = link;
    loop {
        if !visited.insert(current.to_owned()) {
            return Err(Error::UnsafeArchiveEntry(format!(
                "cyclic tar file link `{link}`"
            )));
        }
        let target = links.get(current).ok_or_else(|| {
            Error::UnsafeArchiveEntry(format!("unresolved tar file link `{link}`"))
        })?;
        if directories.contains(target) {
            return Err(Error::UnsafeArchiveEntry(format!(
                "tar link `{link}` targets a directory"
            )));
        }
        if let Some(file) = regular_files.get(target) {
            return Ok(file);
        }
        if links.contains_key(target) {
            current = target;
            continue;
        }
        return Err(Error::UnsafeArchiveEntry(format!(
            "unresolved tar file link `{link}`"
        )));
    }
}

fn copy_bounded(reader: &mut impl Read, output: File, expected: u64, path: &Path) -> Result<()> {
    let mut writer = io::BufWriter::new(output);
    let copied = io::copy(&mut reader.take(expected + 1), &mut writer)
        .map_err(|source| Error::io(path, source))?;
    if copied != expected {
        return Err(Error::ArchiveLimit(format!(
            "entry `{}` length mismatch",
            path.display()
        )));
    }
    writer.flush().map_err(|source| Error::io(path, source))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| Error::io(path, source))
}

fn safe_archive_path(path: &Path) -> Result<PathBuf> {
    let display = path
        .to_str()
        .ok_or_else(|| Error::UnsafeArchiveEntry(path.display().to_string()))?;
    // A trailing slash is the conventional spelling of a directory entry in ZIP
    // and tar files. Normalize exactly one; every actual component must still be
    // portable and unambiguous on Windows, macOS, and Unix.
    let normalized = display.strip_suffix('/').unwrap_or(display);
    if normalized.ends_with('/')
        || path.is_absolute()
        || !is_portable_relative_path(normalized, false)
    {
        return Err(Error::UnsafeArchiveEntry(path.display().to_string()));
    }

    let mut result = PathBuf::new();
    for component in normalized.split('/') {
        result.push(component);
    }
    Ok(result)
}

struct CollisionGuard {
    case_insensitive: bool,
    entries: BTreeMap<String, String>,
    nodes: BTreeMap<String, (String, bool)>,
}

impl CollisionGuard {
    fn new(case_insensitive: bool) -> Self {
        Self {
            case_insensitive,
            entries: BTreeMap::new(),
            nodes: BTreeMap::new(),
        }
    }

    fn insert(&mut self, path: &Path, is_directory: bool) -> Result<()> {
        let safe = safe_archive_path(path)?;
        let original = portable_path(&safe, false);
        let folded = portable_path(&safe, self.case_insensitive);
        if let Some(previous) = self.entries.insert(folded.clone(), original.clone()) {
            return Err(Error::UnsafeArchiveEntry(format!(
                "duplicate or case-folding collision: `{previous}` and `{original}`"
            )));
        }

        let components: Vec<_> = safe.components().collect();
        let mut prefix = PathBuf::new();
        for (index, component) in components.iter().enumerate() {
            prefix.push(component.as_os_str());
            let prefix_original = portable_path(&prefix, false);
            let prefix_folded = portable_path(&prefix, self.case_insensitive);
            let final_component = index + 1 == components.len();
            let desired_directory = !final_component || is_directory;
            if let Some((known_original, known_directory)) = self.nodes.get(&prefix_folded) {
                if known_original != &prefix_original
                    || (!*known_directory && desired_directory)
                    || (final_component && !is_directory)
                {
                    return Err(Error::UnsafeArchiveEntry(format!(
                        "path collision: `{known_original}` and `{original}`"
                    )));
                }
            } else {
                self.nodes
                    .insert(prefix_folded, (prefix_original, desired_directory));
            }
        }
        Ok(())
    }
}

fn portable_path(path: &Path, fold_case: bool) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .map(|component| {
            if fold_case {
                component.to_lowercase()
            } else {
                component.into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn checked_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = safe_archive_path(Path::new(relative))?;
    Ok(root.join(relative))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    if mode != 0 {
        let safe_mode = mode & 0o777;
        fs::set_permissions(path, fs::Permissions::from_mode(safe_mode))
            .map_err(|source| Error::io(path, source))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use zip::write::SimpleFileOptions;

    use super::*;

    fn append_tar_file(archive: &mut tar::Builder<File>, path: &str, contents: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(u64::try_from(contents.len()).unwrap());
        header.set_mode(0o755);
        header.set_cksum();
        archive.append_data(&mut header, path, contents).unwrap();
    }

    fn append_tar_symlink(archive: &mut tar::Builder<File>, path: &str, target: &str) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        archive.append_link(&mut header, path, target).unwrap();
    }

    fn append_global_pax_record(archive: &mut tar::Builder<File>, key: &str, value: &str) {
        let mut length_digits = 1;
        loop {
            let length = length_digits + 3 + key.len() + value.len();
            let actual_digits = length.to_string().len();
            if actual_digits == length_digits {
                let record = format!("{length} {key}={value}\n");
                let mut header = tar::Header::new_ustar();
                header.set_entry_type(tar::EntryType::new(b'g'));
                header.set_size(u64::try_from(record.len()).unwrap());
                header.set_mode(0o644);
                header.set_cksum();
                archive
                    .append_data(&mut header, "pax_global_header", record.as_bytes())
                    .unwrap();
                return;
            }
            length_digits = actual_digits;
        }
    }

    #[test]
    fn rejects_parent_and_absolute_paths() {
        assert!(safe_archive_path(Path::new("../engine")).is_err());
        assert!(safe_archive_path(Path::new("/engine")).is_err());
        assert_eq!(
            safe_archive_path(Path::new("runtime/engine")).unwrap(),
            PathBuf::from("runtime/engine")
        );
    }

    #[test]
    fn rejects_windows_nonportable_archive_paths() {
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
            "runtime/NUL",
            "runtime/prn.txt",
            "runtime/cOm9.exe",
            "runtime/LPT².log",
            "runtime/CONOUT$.txt",
            "runtime/./engine",
            "runtime//engine",
        ] {
            assert!(
                safe_archive_path(Path::new(path)).is_err(),
                "unexpectedly accepted {path}"
            );
        }

        assert_eq!(
            safe_archive_path(Path::new("runtime/models/")).unwrap(),
            PathBuf::from("runtime/models")
        );
    }

    #[test]
    fn rejects_zip_case_fold_collisions() {
        let temporary = tempfile::tempdir().unwrap();
        let archive_path = temporary.path().join("collision.zip");
        let file = File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("Runtime/Engine", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"one").unwrap();
        archive
            .start_file("runtime/engine", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"two").unwrap();
        archive.finish().unwrap();

        let error = extract_zip(
            &archive_path,
            temporary.path(),
            &temporary.path().join("output"),
            &mut ExtractionBudget::with_limits(MAX_EXTRACTED_BYTES, MAX_ARCHIVE_ENTRIES, true),
        )
        .unwrap_err();
        assert!(matches!(error, Error::UnsafeArchiveEntry(_)));
    }

    #[test]
    fn collision_guards_follow_destination_case_sensitivity() {
        let mut sensitive = CollisionGuard::new(false);
        sensitive
            .insert(Path::new("terminfo/2621A"), false)
            .unwrap();
        sensitive
            .insert(Path::new("terminfo/2621a"), false)
            .unwrap();

        let mut insensitive = CollisionGuard::new(true);
        insensitive
            .insert(Path::new("terminfo/2621A"), false)
            .unwrap();
        assert!(
            insensitive
                .insert(Path::new("terminfo/2621a"), false)
                .is_err()
        );

        let temporary = tempfile::tempdir().unwrap();
        let upper = temporary.path().join("Terminfo/2621A");
        let lower = temporary.path().join("Terminfo/2621a");
        let mut budget =
            ExtractionBudget::with_limits(MAX_EXTRACTED_BYTES, MAX_ARCHIVE_ENTRIES, false);
        budget
            .reserve_path(temporary.path(), &upper, false, 1)
            .unwrap();
        budget
            .reserve_path(temporary.path(), &lower, false, 1)
            .unwrap();
        let mut insensitive_budget =
            ExtractionBudget::with_limits(MAX_EXTRACTED_BYTES, MAX_ARCHIVE_ENTRIES, true);
        insensitive_budget
            .reserve_path(temporary.path(), &upper, false, 1)
            .unwrap();
        assert!(
            insensitive_budget
                .reserve_path(temporary.path(), &lower, false, 1)
                .is_err()
        );
    }

    #[test]
    fn accepts_bounded_global_pax_comments() {
        let temporary = tempfile::tempdir().unwrap();
        let archive_path = temporary.path().join("comment.tar");
        let file = File::create(&archive_path).unwrap();
        let mut archive = tar::Builder::new(file);
        append_global_pax_record(&mut archive, "comment", "reviewed source revision");
        append_tar_file(&mut archive, "source/LICENSE", b"license fixture");
        archive.finish().unwrap();

        extract_tar(
            File::open(&archive_path).unwrap(),
            temporary.path(),
            &temporary.path().join("output"),
            &mut ExtractionBudget::for_generation(temporary.path()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            fs::read(temporary.path().join("output/source/LICENSE")).unwrap(),
            b"license fixture"
        );
        assert!(!temporary.path().join("output/pax_global_header").exists());
    }

    #[test]
    fn rejects_behavior_changing_global_pax_metadata() {
        let temporary = tempfile::tempdir().unwrap();
        let archive_path = temporary.path().join("path.tar");
        let file = File::create(&archive_path).unwrap();
        let mut archive = tar::Builder::new(file);
        append_global_pax_record(&mut archive, "path", "../outside");
        append_tar_file(&mut archive, "source/LICENSE", b"license fixture");
        archive.finish().unwrap();

        let error = extract_tar(
            File::open(&archive_path).unwrap(),
            temporary.path(),
            &temporary.path().join("output"),
            &mut ExtractionBudget::for_generation(temporary.path()).unwrap(),
        )
        .unwrap_err();
        assert!(matches!(error, Error::UnsafeArchiveEntry(_)));
        assert!(!temporary.path().join("outside").exists());
    }

    #[test]
    fn rejects_tar_links() {
        let temporary = tempfile::tempdir().unwrap();
        let archive_path = temporary.path().join("link.tar");
        let file = File::create(&archive_path).unwrap();
        let mut archive = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        archive
            .append_link(&mut header, "engine", "/outside")
            .unwrap();
        archive.finish().unwrap();
        let file = File::open(&archive_path).unwrap();

        let error = extract_tar(
            file,
            temporary.path(),
            &temporary.path().join("output"),
            &mut ExtractionBudget::for_generation(temporary.path()).unwrap(),
        )
        .unwrap_err();
        assert!(matches!(error, Error::UnsafeArchiveEntry(_)));
    }

    #[test]
    fn flattens_contained_tar_file_symlinks_after_regular_files() {
        let temporary = tempfile::tempdir().unwrap();
        let archive_path = temporary.path().join("link.tar");
        let file = File::create(&archive_path).unwrap();
        let mut archive = tar::Builder::new(file);
        append_tar_symlink(&mut archive, "python/bin/python", "python3.12");
        append_tar_file(
            &mut archive,
            "python/bin/python3.12",
            b"portable python fixture",
        );
        archive.finish().unwrap();

        extract_tar(
            File::open(&archive_path).unwrap(),
            temporary.path(),
            &temporary.path().join("output"),
            &mut ExtractionBudget::for_generation(temporary.path()).unwrap(),
        )
        .unwrap();

        assert_eq!(
            fs::read(temporary.path().join("output/python/bin/python")).unwrap(),
            b"portable python fixture"
        );
        assert!(
            !fs::symlink_metadata(temporary.path().join("output/python/bin/python"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn flattens_contained_tar_file_symlink_chains() {
        let temporary = tempfile::tempdir().unwrap();
        let archive_path = temporary.path().join("chain.tar");
        let file = File::create(&archive_path).unwrap();
        let mut archive = tar::Builder::new(file);
        append_tar_symlink(&mut archive, "lib/library.so", "library.so.1");
        append_tar_symlink(
            &mut archive,
            "lib/library.so.1",
            "../objects/library.so.1.2",
        );
        append_tar_file(
            &mut archive,
            "objects/library.so.1.2",
            b"shared library fixture",
        );
        archive.finish().unwrap();

        extract_tar(
            File::open(&archive_path).unwrap(),
            temporary.path(),
            &temporary.path().join("output"),
            &mut ExtractionBudget::for_generation(temporary.path()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            fs::read(temporary.path().join("output/lib/library.so")).unwrap(),
            b"shared library fixture"
        );
        assert_eq!(
            fs::read(temporary.path().join("output/lib/library.so.1")).unwrap(),
            b"shared library fixture"
        );
    }

    #[test]
    fn rejects_escaping_unresolved_directory_and_cyclic_tar_links() {
        for (name, links, directory) in [
            ("escaping", vec![("link", "../outside")], None),
            ("unresolved", vec![("link", "missing")], None),
            ("directory", vec![("link", "target")], Some("target")),
            (
                "cyclic",
                vec![("first", "second"), ("second", "first")],
                None,
            ),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let archive_path = temporary.path().join(format!("{name}.tar"));
            let file = File::create(&archive_path).unwrap();
            let mut archive = tar::Builder::new(file);
            for (path, target) in links {
                append_tar_symlink(&mut archive, path, target);
            }
            if let Some(path) = directory {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_mode(0o755);
                header.set_cksum();
                archive.append_data(&mut header, path, io::empty()).unwrap();
            }
            archive.finish().unwrap();

            let error = extract_tar(
                File::open(&archive_path).unwrap(),
                temporary.path(),
                &temporary.path().join("output"),
                &mut ExtractionBudget::for_generation(temporary.path()).unwrap(),
            )
            .unwrap_err();
            assert!(
                matches!(error, Error::UnsafeArchiveEntry(_)),
                "unexpected error for {name}: {error}"
            );
        }
    }

    #[test]
    fn flattened_tar_links_consume_byte_and_entry_budgets() {
        let temporary = tempfile::tempdir().unwrap();
        let archive_path = temporary.path().join("budget.tar");
        let file = File::create(&archive_path).unwrap();
        let mut archive = tar::Builder::new(file);
        append_tar_file(&mut archive, "target", b"1234");
        append_tar_symlink(&mut archive, "alias", "target");
        archive.finish().unwrap();

        let mut budget = ExtractionBudget::with_limits(7, MAX_ARCHIVE_ENTRIES, false);
        let error = extract_tar(
            File::open(&archive_path).unwrap(),
            temporary.path(),
            &temporary.path().join("output"),
            &mut budget,
        )
        .unwrap_err();
        assert!(matches!(error, Error::ArchiveLimit(_)));
        assert_eq!(budget.bytes_used, 4);
        assert_eq!(budget.entries_used, 2);
        assert!(!temporary.path().join("output/alias").exists());
    }

    #[test]
    #[ignore = "set UCI_GRABBER_PYTHON_ARCHIVE to a verified python-build-standalone tar.gz"]
    fn extracts_real_python_build_standalone_archive() {
        let archive_path = PathBuf::from(
            std::env::var_os("UCI_GRABBER_PYTHON_ARCHIVE")
                .expect("UCI_GRABBER_PYTHON_ARCHIVE must identify the verified tar.gz"),
        );
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("output");
        let file = File::open(&archive_path).unwrap();
        let mut budget = ExtractionBudget::for_generation(temporary.path()).unwrap();
        extract_tar(GzDecoder::new(file), temporary.path(), &output, &mut budget).unwrap();

        let unix_python = output.join("python/bin/python3.12");
        let windows_python = output.join("python/python.exe");
        let python = if unix_python.is_file() {
            unix_python
        } else {
            windows_python
        };
        let metadata = fs::symlink_metadata(&python).unwrap();
        assert!(metadata.file_type().is_file());
        assert!(!metadata.file_type().is_symlink());
        let snapshot = crate::registry::package_snapshot(&output).unwrap();
        assert!(snapshot.file_count > 0);
        assert!(snapshot.byte_count > 0);
    }

    #[test]
    fn cumulative_budget_rejects_later_artifact_without_large_allocation() {
        let temporary = tempfile::tempdir().unwrap();
        let first_source = temporary.path().join("first.download");
        let second_source = temporary.path().join("second.download");
        fs::write(&first_source, [0_u8; 6]).unwrap();
        fs::write(&second_source, [0_u8; 5]).unwrap();
        let artifact = |destination: &str, byte_count| Artifact {
            kind: crate::schema::ArtifactKind::Other,
            url: "https://example.test/artifact".into(),
            byte_count,
            sha256: "00".repeat(32),
            format: ArchiveFormat::Raw,
            destination: destination.into(),
        };
        let mut budget = ExtractionBudget::with_limits(10, MAX_ARCHIVE_ENTRIES, false);
        let output = temporary.path().join("output");
        materialize(&artifact("first", 6), &first_source, &output, &mut budget).unwrap();
        let error =
            materialize(&artifact("second", 5), &second_source, &output, &mut budget).unwrap_err();

        assert!(matches!(error, Error::ArchiveLimit(_)));
        assert_eq!(budget.bytes_used, 6);
        assert_eq!(fs::read(output.join("first")).unwrap(), [0_u8; 6]);
        assert!(!output.join("second").exists());
    }

    #[test]
    fn cumulative_entry_budget_counts_files_and_implicit_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("artifact.download");
        fs::write(&source, [0_u8; 1]).unwrap();
        let artifact = |destination: &str| Artifact {
            kind: crate::schema::ArtifactKind::Other,
            url: "https://example.test/artifact".into(),
            byte_count: 1,
            sha256: "00".repeat(32),
            format: ArchiveFormat::Raw,
            destination: destination.into(),
        };
        let output = temporary.path().join("output");
        fs::create_dir(&output).unwrap();
        let mut budget = ExtractionBudget::with_limits(MAX_EXTRACTED_BYTES, 2, false);
        materialize(&artifact("first"), &source, &output, &mut budget).unwrap();
        let error =
            materialize(&artifact("nested/second"), &source, &output, &mut budget).unwrap_err();

        assert!(matches!(error, Error::ArchiveLimit(_)));
        assert_eq!(budget.entries_used, 1);
        assert!(!output.join("nested").exists());
    }
}
