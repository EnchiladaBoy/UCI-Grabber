use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::schema::{MAX_MANIFEST_BYTES, Recipe};
use crate::{Error, Result};

#[derive(Clone, Debug)]
pub struct CustomRecipeStore {
    directory: PathBuf,
}

impl CustomRecipeStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn import(&self, source: &str) -> Result<Recipe> {
        let bytes = if source.starts_with("https://") {
            download_manifest(source)?
        } else {
            read_manifest(Path::new(source))?
        };
        let recipe = Recipe::from_json(&bytes)?;
        self.save(&recipe)?;
        Ok(recipe)
    }

    pub fn save(&self, recipe: &Recipe) -> Result<()> {
        recipe.validate()?;
        let recipe_dir = self.directory.join(&recipe.id);
        fs::create_dir_all(&recipe_dir).map_err(|source| Error::io(&recipe_dir, source))?;
        let destination = recipe_dir.join(format!("{}.json", recipe.version));
        let canonical = serde_json::to_vec(recipe)?;
        if destination.exists() {
            return ensure_immutable_match(&destination, recipe, &canonical);
        }

        let mut temporary = tempfile::NamedTempFile::new_in(&recipe_dir)
            .map_err(|source| Error::io(&recipe_dir, source))?;
        temporary
            .write_all(&canonical)
            .and_then(|()| temporary.write_all(b"\n"))
            .map_err(|source| Error::io(temporary.path(), source))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| Error::io(temporary.path(), source))?;
        match temporary.persist_noclobber(&destination) {
            Ok(file) => file
                .sync_all()
                .map_err(|source| Error::io(&destination, source)),
            Err(_error) if destination.exists() => {
                ensure_immutable_match(&destination, recipe, &canonical)
            }
            Err(error) => Err(Error::io(&destination, error.error)),
        }
    }

    pub fn load_all(&self) -> Result<Vec<Recipe>> {
        if !self.directory.exists() {
            return Ok(Vec::new());
        }
        let mut recipes = Vec::new();
        for recipe_dir in
            fs::read_dir(&self.directory).map_err(|source| Error::io(&self.directory, source))?
        {
            let recipe_dir = recipe_dir.map_err(|source| Error::io(&self.directory, source))?;
            if !recipe_dir
                .file_type()
                .map_err(|source| Error::io(recipe_dir.path(), source))?
                .is_dir()
            {
                continue;
            }
            for entry in fs::read_dir(recipe_dir.path())
                .map_err(|source| Error::io(recipe_dir.path(), source))?
            {
                let entry = entry.map_err(|source| Error::io(recipe_dir.path(), source))?;
                if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                recipes.push(Recipe::from_json(&read_manifest(&entry.path())?)?);
            }
        }
        recipes.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.version.cmp(&right.version))
        });
        Ok(recipes)
    }
}

fn ensure_immutable_match(
    destination: &Path,
    recipe: &Recipe,
    expected_canonical: &[u8],
) -> Result<()> {
    let metadata =
        fs::symlink_metadata(destination).map_err(|source| Error::io(destination, source))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::InvalidRecipe(format!(
            "custom recipe {} {} is not a regular immutable manifest",
            recipe.id, recipe.version
        )));
    }
    let existing = Recipe::from_json(&read_manifest(destination)?)?;
    if serde_json::to_vec(&existing)? == expected_canonical {
        Ok(())
    } else {
        Err(Error::InvalidRecipe(format!(
            "custom recipe {} {} is immutable; import changed content under a new version",
            recipe.id, recipe.version
        )))
    }
}

fn read_manifest(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path).map_err(|source| Error::io(path, source))?;
    if metadata.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(Error::InvalidRecipe(format!(
            "manifest exceeds {MAX_MANIFEST_BYTES} bytes"
        )));
    }
    fs::read(path).map_err(|source| Error::io(path, source))
}

fn download_manifest(url: &str) -> Result<Vec<u8>> {
    let config = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .max_redirects_will_error(true)
        .timeout_global(Some(Duration::from_secs(60)))
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
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| Error::Download {
            url: url.into(),
            message: source.to_string(),
        })?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(Error::InvalidRecipe(format!(
            "manifest exceeds {MAX_MANIFEST_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_changed_same_version_import_and_preserves_original() {
        let temporary = tempfile::tempdir().unwrap();
        let store = CustomRecipeStore::new(temporary.path().join("store"));
        let original = Recipe::from_json(include_bytes!(
            "../catalog/tests/fixtures/valid-recipe.json"
        ))
        .unwrap();
        let mut changed = original.clone();
        changed.description = "Different package description".into();

        let original_source = temporary.path().join("original.json");
        let changed_source = temporary.path().join("changed.json");
        fs::write(
            &original_source,
            serde_json::to_vec_pretty(&original).unwrap(),
        )
        .unwrap();
        fs::write(
            &changed_source,
            serde_json::to_vec_pretty(&changed).unwrap(),
        )
        .unwrap();

        store.import(original_source.to_str().unwrap()).unwrap();
        store.import(original_source.to_str().unwrap()).unwrap();
        let stored_path = temporary
            .path()
            .join("store")
            .join(&original.id)
            .join(format!("{}.json", original.version));
        let before = fs::read(&stored_path).unwrap();

        let error = store.import(changed_source.to_str().unwrap()).unwrap_err();
        assert!(error.to_string().contains("immutable"));
        assert_eq!(fs::read(stored_path).unwrap(), before);
        assert_eq!(store.load_all().unwrap(), vec![original]);
    }
}
