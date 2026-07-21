use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use eframe::egui::{self, Color32, RichText};

use crate::catalog::{
    CatalogCache, CatalogClient, VerifiedCatalog, bundled_catalog, default_client,
};
use crate::handoff;
use crate::install::{InstallOptions, Installer};
use crate::recipes::CustomRecipeStore;
use crate::registry::{InstallRecord, InstallSource, Integrity, RegistryStore};
use crate::schema::{Recipe, current_platform};
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Catalog,
    Installed,
    Custom,
}

enum JobResult {
    Installed(Result<InstallRecord>),
    Refreshed(Result<VerifiedCatalog>),
    Imported(Result<Recipe>),
}

pub struct GrabberApp {
    installer: Installer,
    catalog_client: CatalogClient,
    catalog: VerifiedCatalog,
    custom_store: CustomRecipeStore,
    custom_recipes: Vec<Recipe>,
    installs: Vec<InstallRecord>,
    integrity: BTreeMap<String, String>,
    tab: Tab,
    status: String,
    custom_url: String,
    approved_unreviewed: BTreeSet<String>,
    confirm_remove: Option<String>,
    job: Option<mpsc::Receiver<JobResult>>,
    job_label: Option<String>,
    job_cancel: Option<Arc<AtomicBool>>,
}

impl GrabberApp {
    pub fn load() -> Result<Self> {
        let store = RegistryStore::open_default()?;
        let installer = Installer::default_store()?;
        let recovery = installer.recover()?;
        let catalog_client = default_client(CatalogCache::new(store.cache_dir()))?;
        let (catalog, mut status) = match catalog_client.cached() {
            Ok(Some(catalog)) => (catalog, "Loaded verified catalog cache.".to_owned()),
            Ok(None) => (
                bundled_catalog()?,
                "Using the signed bootstrap catalog. Refresh to check for engines.".to_owned(),
            ),
            Err(error) => (
                bundled_catalog()?,
                format!("Ignored invalid catalog cache: {error}"),
            ),
        };
        if recovery.cleaned_staging > 0 || recovery.repaired_records > 0 {
            status = format!(
                "Recovered {} interrupted staging director{} and {} registry record{}.",
                recovery.cleaned_staging,
                if recovery.cleaned_staging == 1 {
                    "y"
                } else {
                    "ies"
                },
                recovery.repaired_records,
                if recovery.repaired_records == 1 {
                    ""
                } else {
                    "s"
                }
            );
        }
        let custom_store = CustomRecipeStore::new(store.data_root().join("custom-recipes"));
        let custom_recipes = custom_store.load_all()?;
        let installs = store.load()?.installs;
        let integrity = integrity_labels(&installs);
        Ok(Self {
            installer,
            catalog_client,
            catalog,
            custom_store,
            custom_recipes,
            installs,
            integrity,
            tab: Tab::Catalog,
            status,
            custom_url: String::new(),
            approved_unreviewed: BTreeSet::new(),
            confirm_remove: None,
            job: None,
            job_label: None,
            job_cancel: None,
        })
    }

    fn reload_installs(&mut self) {
        match self.installer.store().load() {
            Ok(registry) => {
                self.installs = registry.installs;
                self.integrity = integrity_labels(&self.installs);
            }
            Err(error) => self.status = format!("Could not reload installs: {error}"),
        }
    }

    fn reload_custom(&mut self) {
        match self.custom_store.load_all() {
            Ok(recipes) => self.custom_recipes = recipes,
            Err(error) => self.status = format!("Could not reload custom recipes: {error}"),
        }
    }

    fn start_install(&mut self, recipe: Recipe, model_id: String, source: InstallSource) {
        if self.job.is_some() {
            return;
        }
        let installer = self.installer.clone();
        let approval_key = format!("{}:{}", recipe.id, recipe.version);
        let approve_unreviewed = self.approved_unreviewed.contains(&approval_key);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let label = format!("Installing {}…", recipe.name);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let options = InstallOptions {
                source,
                approve_unreviewed,
                ..InstallOptions::default()
            };
            let result = installer.install(&recipe, &model_id, &options, &worker_cancel);
            let _ = sender.send(JobResult::Installed(result));
        });
        self.job = Some(receiver);
        self.job_label = Some(label.clone());
        self.job_cancel = Some(cancel);
        self.status = label;
    }

    fn start_refresh(&mut self) {
        if self.job.is_some() {
            return;
        }
        let client = self.catalog_client.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(JobResult::Refreshed(client.refresh()));
        });
        self.job = Some(receiver);
        self.job_label = Some("Refreshing signed catalog…".into());
        self.job_cancel = None;
        self.status = "Refreshing signed catalog…".into();
    }

    fn start_import(&mut self, source: String) {
        if self.job.is_some() {
            return;
        }
        let store = self.custom_store.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(JobResult::Imported(store.import(&source)));
        });
        self.job = Some(receiver);
        self.job_label = Some("Importing unreviewed recipe…".into());
        self.job_cancel = None;
        self.status = "Importing and validating unreviewed recipe…".into();
    }

    fn poll_job(&mut self) {
        let Some(receiver) = &self.job else { return };
        let Ok(result) = receiver.try_recv() else {
            return;
        };
        self.job = None;
        self.job_label = None;
        self.job_cancel = None;
        match result {
            JobResult::Installed(Ok(record)) => {
                self.status = format!("Installed and validated {}.", record.name);
                self.tab = Tab::Installed;
                self.reload_installs();
            }
            JobResult::Installed(Err(error)) => {
                self.status = format!("Install failed: {error}");
            }
            JobResult::Refreshed(Ok(catalog)) => {
                let count = catalog.catalog.recipes.len();
                self.catalog = catalog;
                self.status = format!("Verified catalog refreshed ({count} recipes).");
            }
            JobResult::Refreshed(Err(error)) => {
                self.status = format!("Catalog refresh failed; kept previous catalog: {error}");
            }
            JobResult::Imported(Ok(recipe)) => {
                self.approved_unreviewed
                    .remove(&format!("{}:{}", recipe.id, recipe.version));
                self.status = format!("Imported unreviewed recipe {}.", recipe.name);
                self.reload_custom();
            }
            JobResult::Imported(Err(error)) => {
                self.status = format!("Recipe import failed: {error}");
            }
        }
    }

    fn recipe_list(&mut self, ui: &mut egui::Ui, recipes: &[Recipe], source: InstallSource) {
        let platform = current_platform().unwrap_or("unsupported");
        let mut requested = None;
        for recipe in recipes {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading(format!("{} {}", recipe.name, recipe.version));
                ui.label(&recipe.description);
                ui.small(format!(
                    "Publisher: {}  •  License: {}  •  Minimum FishEye: {}",
                    recipe.publisher.name, recipe.license.spdx, recipe.minimum_fisheye_version
                ));
                egui::CollapsingHeader::new("Source and package details")
                    .id_salt((source, &recipe.id, &recipe.version))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.hyperlink_to("Homepage", &recipe.homepage);
                            ui.hyperlink_to("Publisher", &recipe.publisher.url);
                            ui.hyperlink_to("License", &recipe.license.url);
                            ui.hyperlink_to("Source", &recipe.license.source_url);
                        });
                        for model in &recipe.models {
                            let Some(package) = model
                                .packages
                                .iter()
                                .find(|package| package.platform == platform)
                            else {
                                continue;
                            };
                            ui.strong(format!("{} · {}", model.name, package.platform));
                            for artifact in &package.artifacts {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(format!(
                                        "{:?} · {} bytes · {}",
                                        artifact.kind, artifact.byte_count, artifact.destination
                                    ));
                                    ui.hyperlink_to("Artifact URL", &artifact.url);
                                });
                                ui.monospace(format!("SHA-256  {}", artifact.sha256));
                            }
                        }
                    });
                if source == InstallSource::UnreviewedRecipe {
                    let approval_key = format!("{}:{}", recipe.id, recipe.version);
                    let mut approved = self.approved_unreviewed.contains(&approval_key);
                    if ui.checkbox(
                        &mut approved,
                        "I understand testing runs this unreviewed native executable with my account permissions and no OS sandbox",
                    ).changed() {
                        if approved {
                            self.approved_unreviewed.insert(approval_key);
                        } else {
                            self.approved_unreviewed.remove(&approval_key);
                        }
                    }
                }
                ui.add_space(6.0);
                for model in &recipe.models {
                    let supported = model
                        .packages
                        .iter()
                        .any(|package| package.platform == platform);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.strong(&model.name);
                            ui.small(&model.description);
                        });
                        ui.add_space(12.0);
                        let enabled = supported
                            && self.job.is_none()
                            && (source == InstallSource::Curated
                                || self.approved_unreviewed.contains(&format!(
                                    "{}:{}",
                                    recipe.id, recipe.version
                                )));
                        if ui
                            .add_enabled(enabled, egui::Button::new("Install"))
                            .clicked()
                        {
                            requested = Some((recipe.clone(), model.id.clone()));
                        }
                        if !supported {
                            ui.small(format!("Not available for {platform}"));
                        }
                    });
                }
            });
            ui.add_space(8.0);
        }
        if let Some((recipe, model)) = requested {
            self.start_install(recipe, model, source);
        }
    }

    fn catalog_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Curated engines");
            if ui
                .add_enabled(self.job.is_none(), egui::Button::new("Refresh catalog"))
                .clicked()
            {
                self.start_refresh();
            }
        });
        ui.label("Catalog entries are authenticated with a bundled Ed25519 public key. Every package is checksum-verified and tested as UCI before activation.");
        ui.add_space(10.0);
        let recipes = self.catalog.catalog.recipes.clone();
        if recipes.is_empty() {
            ui.label("No curated engines are published in this signed catalog yet. You can import a data-only recipe in Custom Recipes.");
        } else {
            self.recipe_list(ui, &recipes, InstallSource::Curated);
        }
    }

    fn custom_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Custom recipes");
        ui.colored_label(
            Color32::from_rgb(220, 160, 45),
            "Custom recipes are unreviewed. Inspect their publisher, license, URLs, and hashes before approval.",
        );
        let mut import_source = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.job.is_none(), egui::Button::new("Import recipe file…"))
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSON recipe", &["json"])
                    .pick_file()
            {
                import_source = Some(path.to_string_lossy().into_owned());
            }
            ui.label("or HTTPS URL:");
            ui.text_edit_singleline(&mut self.custom_url);
            if ui
                .add_enabled(
                    self.job.is_none() && self.custom_url.starts_with("https://"),
                    egui::Button::new("Import URL"),
                )
                .clicked()
            {
                import_source = Some(std::mem::take(&mut self.custom_url));
            }
        });
        if let Some(source) = import_source {
            self.start_import(source);
        }
        ui.add_space(10.0);
        let recipes = self.custom_recipes.clone();
        if recipes.is_empty() {
            ui.label("No custom recipes imported.");
        } else {
            self.recipe_list(ui, &recipes, InstallSource::UnreviewedRecipe);
        }
    }

    fn installed_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Installed generations");
        ui.label("Updates install beside existing versions. FishEye independently fingerprints, tests, and asks you to approve every executable.");
        ui.add_space(10.0);
        let records = self.installs.clone();
        if records.is_empty() {
            ui.label("No UCI packages installed.");
            return;
        }
        let mut removal = None;
        for record in records {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading(format!("{} {}", record.name, record.version));
                let integrity = self
                    .integrity
                    .get(&record.install_id)
                    .map_or("Not checked", String::as_str);
                ui.label(format!("Integrity: {integrity}"));
                ui.small(format!(
                    "{}  •  {}  •  {}  •  {}",
                    record.source, record.platform, record.publisher, record.license_spdx
                ));
                ui.monospace(record.executable.display().to_string());
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Use in FishEye").clicked() {
                        match RegistryStore::integrity(&record) {
                            Ok(Integrity::Verified) => {
                                match handoff::open_in_fisheye(&record.executable, None) {
                                    Ok(_) => {
                                        self.status =
                                            "Opened FishEye's external engine manager.".into();
                                    }
                                    Err(error) => {
                                        self.status = format!(
                                            "Could not open FishEye: {error}. Copy the path or reveal it instead."
                                        );
                                    }
                                }
                            }
                            Ok(Integrity::Missing | Integrity::Changed { .. }) => {
                                self.status = "Refused handoff because the installed executable is missing or changed. Copy/Reveal remain available for inspection.".into();
                            }
                            Err(error) => self.status = format!("Integrity check failed: {error}"),
                        }
                    }
                    if ui.button("Copy path").clicked() {
                        ui.ctx().copy_text(record.executable.display().to_string());
                        self.status = "Engine path copied.".into();
                    }
                    if ui.button("Reveal").clicked() {
                        match handoff::reveal(&record.executable) {
                            Ok(_) => self.status = "Revealed installed engine.".into(),
                            Err(error) => self.status = format!("Could not reveal engine: {error}"),
                        }
                    }
                    if ui.button("Verify integrity").clicked() {
                        let label = integrity_label(&record);
                        self.integrity.insert(record.install_id.clone(), label);
                    }
                    if self.confirm_remove.as_deref() == Some(&record.install_id) {
                        if ui
                            .button(RichText::new("Confirm remove").color(Color32::RED))
                            .clicked()
                        {
                            removal = Some(record.install_id.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_remove = None;
                        }
                    } else if ui.button("Remove…").clicked() {
                        self.confirm_remove = Some(record.install_id.clone());
                    }
                });
            });
            ui.add_space(8.0);
        }
        if let Some(install_id) = removal {
            match self.installer.store().remove(&install_id) {
                Ok(true) => self.status = "Removed immutable install generation.".into(),
                Ok(false) => self.status = "Install was already absent.".into(),
                Err(error) => self.status = format!("Remove failed: {error}"),
            }
            self.confirm_remove = None;
            self.reload_installs();
        }
    }
}

impl eframe::App for GrabberApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_job();
        if self.job.is_some() {
            context.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("UCI Grabber");
                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Catalog, "Catalog");
                ui.selectable_value(&mut self.tab, Tab::Installed, "Installed");
                ui.selectable_value(&mut self.tab, Tab::Custom, "Custom Recipes");
                if let Some(label) = &self.job_label {
                    ui.spinner();
                    ui.label(label);
                    if let Some(cancel) = &self.job_cancel
                        && ui.button("Cancel").clicked()
                    {
                        cancel.store(true, Ordering::Relaxed);
                        self.status = "Cancelling install…".into();
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                Tab::Catalog => self.catalog_ui(ui),
                Tab::Installed => self.installed_ui(ui),
                Tab::Custom => self.custom_ui(ui),
            });
            ui.separator();
            ui.label(&self.status);
        });
    }
}

pub fn run_gui() -> Result<()> {
    let app = GrabberApp::load()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([940.0, 700.0])
            .with_min_inner_size([720.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "UCI Grabber",
        options,
        Box::new(move |_context| Ok(Box::new(app))),
    )
    .map_err(|error| Error::Other(format!("GUI failed: {error}")))
}

fn integrity_labels(records: &[InstallRecord]) -> BTreeMap<String, String> {
    records
        .iter()
        .map(|record| {
            (
                record.install_id.clone(),
                if record.executable.is_file() {
                    "Not checked".into()
                } else {
                    "Missing".into()
                },
            )
        })
        .collect()
}

fn integrity_label(record: &InstallRecord) -> String {
    match RegistryStore::integrity(record) {
        Ok(Integrity::Verified) => "Verified".into(),
        Ok(Integrity::Missing) => "Missing".into(),
        Ok(Integrity::Changed { .. }) => "Changed — do not launch".into(),
        Err(error) => format!("Check failed: {error}"),
    }
}
