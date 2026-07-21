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
use crate::install::{InstallOptions, InstallPhase, InstallProgress, Installer};
use crate::recipes::CustomRecipeStore;
use crate::registry::{InstallRecord, InstallSource, Integrity, RegistryStore};
use crate::schema::{ArtifactKind, Model, Recipe, current_platform};
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Catalog,
    Installed,
    Custom,
}

enum JobResult {
    Installed(Result<InstallRecord>),
    InstallProgress(InstallProgress),
    Refreshed(Result<VerifiedCatalog>),
    Imported(Result<Recipe>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PostInstallAction {
    ShowReady,
    OpenInFishEye,
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
    recent_install: Option<String>,
    job: Option<mpsc::Receiver<JobResult>>,
    job_label: Option<String>,
    job_cancel: Option<Arc<AtomicBool>>,
    install_progress: Option<InstallProgress>,
    catalog_refreshing: bool,
    post_install_action: PostInstallAction,
}

impl GrabberApp {
    pub fn load() -> Result<Self> {
        let store = RegistryStore::open_default()?;
        let installer = Installer::default_store()?;
        let recovery = installer.recover()?;
        let catalog_client = default_client(CatalogCache::new(store.cache_dir()))?;
        let (catalog, mut status, refresh_on_launch) = match catalog_client.cached() {
            Ok(Some(catalog)) => (catalog, "Loaded verified catalog cache.".to_owned(), false),
            Ok(None) => (
                bundled_catalog()?,
                "No verified catalog cache yet.".to_owned(),
                true,
            ),
            Err(error) => (
                bundled_catalog()?,
                format!("Ignored invalid catalog cache: {error}"),
                true,
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
        let mut app = Self {
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
            recent_install: None,
            job: None,
            job_label: None,
            job_cancel: None,
            install_progress: None,
            catalog_refreshing: false,
            post_install_action: PostInstallAction::ShowReady,
        };
        if refresh_on_launch {
            let launch_status = app.status.clone();
            app.start_refresh();
            app.status = format!("{launch_status} Downloading the latest signed catalog…");
        }
        Ok(app)
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

    fn start_install(
        &mut self,
        recipe: Recipe,
        model_id: String,
        source: InstallSource,
        post_install_action: PostInstallAction,
    ) {
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
            let progress_sender = sender.clone();
            let result = installer.install_with_progress(
                &recipe,
                &model_id,
                &options,
                &worker_cancel,
                &move |progress| {
                    let _ = progress_sender.send(JobResult::InstallProgress(progress));
                },
            );
            let _ = sender.send(JobResult::Installed(result));
        });
        self.job = Some(receiver);
        self.job_label = Some(label.clone());
        self.job_cancel = Some(cancel);
        self.install_progress = None;
        self.post_install_action = post_install_action;
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
        self.catalog_refreshing = true;
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
        let mut terminal = None;
        if let Some(receiver) = &self.job {
            while let Ok(result) = receiver.try_recv() {
                match result {
                    JobResult::InstallProgress(progress) => {
                        self.install_progress = Some(progress);
                    }
                    result => {
                        terminal = Some(result);
                        break;
                    }
                }
            }
        }
        let Some(result) = terminal else {
            return;
        };
        self.job = None;
        self.job_label = None;
        self.job_cancel = None;
        self.install_progress = None;
        let post_install_action = self.post_install_action;
        self.post_install_action = PostInstallAction::ShowReady;
        match result {
            JobResult::Installed(Ok(record)) => {
                self.recent_install = Some(record.install_id.clone());
                self.tab = Tab::Installed;
                self.reload_installs();
                self.status = format!(
                    "UCI engine ready: {}. Use the executable path in any compatible chess GUI.",
                    record.name
                );
                if post_install_action == PostInstallAction::OpenInFishEye {
                    self.open_verified_record_in_fisheye(&record);
                }
            }
            JobResult::Installed(Err(error)) => {
                self.status = format!("Install failed: {error}");
            }
            JobResult::Refreshed(Ok(catalog)) => {
                self.catalog_refreshing = false;
                let count = catalog.catalog.recipes.len();
                self.catalog = catalog;
                self.status = format!("Verified catalog refreshed ({count} recipes).");
            }
            JobResult::Refreshed(Err(error)) => {
                self.catalog_refreshing = false;
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
            JobResult::InstallProgress(_) => unreachable!("progress messages are drained above"),
        }
    }

    fn open_record_in_fisheye(&mut self, record: &InstallRecord) {
        match RegistryStore::integrity(record) {
            Ok(Integrity::Verified) => {}
            Ok(Integrity::Missing | Integrity::Changed { .. }) => {
                self.status = "Refused FishEye handoff because the UCI package is missing or changed. Copy path and Open folder remain available for inspection.".into();
                return;
            }
            Err(error) => {
                self.status = format!("Integrity check failed: {error}");
                return;
            }
        }

        self.open_verified_record_in_fisheye(record);
    }

    fn open_verified_record_in_fisheye(&mut self, record: &InstallRecord) {
        let fisheye = match handoff::find_fisheye(None) {
            Ok(path) => path,
            Err(discovery_error) => {
                let Some(path) = pick_fisheye_executable() else {
                    self.status = format!(
                        "UCI engine ready. FishEye was not found ({discovery_error}) and no executable was selected; use Copy engine path for any chess GUI."
                    );
                    return;
                };
                path
            }
        };
        match handoff::open_in_fisheye(&record.executable, Some(&fisheye)) {
            Ok(_) => {
                self.status = "Opened FishEye's external-engine review. FishEye will test the UCI engine and ask before saving it.".into();
            }
            Err(error) => {
                self.status = format!(
                    "UCI engine ready, but FishEye could not be opened: {error}. Use Copy engine path or Open folder instead."
                );
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
                    "Publisher: {}  •  License: {} ({})",
                    recipe.publisher.name, recipe.license.name, recipe.license.spdx
                ));
                ui.small(format!(
                    "Optional FishEye integration: version {} or newer",
                    recipe.minimum_fisheye_version
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
                                    let artifact_label = match artifact.kind {
                                        ArtifactKind::Runtime => "Runtime",
                                        ArtifactKind::Model => "Checkpoint",
                                        ArtifactKind::Other => "Package file",
                                    };
                                    ui.label(format!(
                                        "{artifact_label} · {} · {}",
                                        format_bytes(artifact.byte_count),
                                        artifact.destination
                                    ));
                                    ui.hyperlink_to("Download source", &artifact.url);
                                    if artifact.kind == ArtifactKind::Model
                                        && let Some(model_card) =
                                            hugging_face_model_card_url(&artifact.url)
                                    {
                                        ui.hyperlink_to("Pinned model card / terms", model_card);
                                    }
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
                    let package = model
                        .packages
                        .iter()
                        .find(|package| package.platform == platform);
                    let supported = package.is_some();
                    ui.horizontal_wrapped(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.strong(&model.name);
                                if let Some(guidance) = model_guidance(model) {
                                    let color = if guidance == "Recommended" {
                                        Color32::from_rgb(60, 155, 90)
                                    } else {
                                        ui.visuals().weak_text_color()
                                    };
                                    ui.label(RichText::new(guidance).color(color).strong());
                                }
                            });
                            ui.small(&model.description);
                            if let Some(package) = package {
                                let total = package
                                    .artifacts
                                    .iter()
                                    .map(|artifact| artifact.byte_count)
                                    .sum::<u64>();
                                let model_size = package
                                    .artifacts
                                    .iter()
                                    .filter(|artifact| artifact.kind == ArtifactKind::Model)
                                    .map(|artifact| artifact.byte_count)
                                    .sum::<u64>();
                                if model_size > 0 {
                                    ui.small(format!(
                                        "{} model • {} total download",
                                        format_bytes(model_size),
                                        format_bytes(total)
                                    ));
                                } else {
                                    ui.small(format!("{} total download", format_bytes(total)));
                                }
                            }
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
                            requested = Some((
                                recipe.clone(),
                                model.id.clone(),
                                PostInstallAction::ShowReady,
                            ));
                        }
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new("Install & open in FishEye"),
                            )
                            .on_hover_text(
                                "Optional: opens FishEye's review flow after the UCI engine is ready",
                            )
                            .clicked()
                        {
                            requested = Some((
                                recipe.clone(),
                                model.id.clone(),
                                PostInstallAction::OpenInFishEye,
                            ));
                        }
                        if !supported {
                            ui.small(format!("Not available for {platform}"));
                        }
                    });
                }
            });
            ui.add_space(8.0);
        }
        if let Some((recipe, model, post_install_action)) = requested {
            self.start_install(recipe, model, source, post_install_action);
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
            if self.catalog_refreshing {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading and verifying the latest curated catalogue…");
                });
            } else {
                ui.label("No curated engines are published in this signed catalog yet. You can import a data-only recipe in Custom Recipes.");
            }
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
        ui.heading("Ready UCI engines");
        ui.label("Each verified install is a self-contained, portable UCI package. Copy its executable path into any compatible chess GUI, and keep the package folder together.");
        ui.add_space(10.0);
        let records = self.installs.clone();
        if records.is_empty() {
            ui.label("No UCI packages installed.");
            return;
        }
        let recent = self.recent_install.as_ref().and_then(|install_id| {
            records
                .iter()
                .find(|record| &record.install_id == install_id)
                .cloned()
        });
        let mut open_recent_in_fisheye = false;
        if let Some(record) = &recent {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading(
                    RichText::new("UCI engine ready")
                        .color(Color32::from_rgb(60, 155, 90)),
                );
                ui.label(format!(
                    "{} ({}) passed checksum and UCI validation.",
                    record.name, record.model_id
                ));
                ui.monospace(record.executable.display().to_string());
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Copy engine path").clicked() {
                        ui.ctx().copy_text(record.executable.display().to_string());
                        self.status = "Engine path copied for use in any chess GUI.".into();
                    }
                    if ui.button("Open package folder").clicked() {
                        match handoff::reveal(&record.executable) {
                            Ok(_) => self.status = "Opened the portable UCI package folder.".into(),
                            Err(error) => {
                                self.status = format!("Could not open package folder: {error}");
                            }
                        }
                    }
                    if ui.button("Open review in FishEye").clicked() {
                        open_recent_in_fisheye = true;
                    }
                    if ui.small_button("Dismiss").clicked() {
                        self.recent_install = None;
                    }
                });
                ui.small("FishEye integration is optional and opens its review screen; UCI Grabber never approves or writes FishEye settings.");
            });
            ui.add_space(10.0);
        }
        if open_recent_in_fisheye {
            self.open_verified_record_in_fisheye(recent.as_ref().expect("recent record exists"));
        }
        let mut removal = None;
        for record in records {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading(format!(
                    "{} · {} · {}",
                    record.name, record.model_id, record.version
                ));
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
                    if ui.button("Copy engine path").clicked() {
                        ui.ctx().copy_text(record.executable.display().to_string());
                        self.status = "Engine path copied for use in any chess GUI.".into();
                    }
                    if ui.button("Open package folder").clicked() {
                        match handoff::reveal(&record.executable) {
                            Ok(_) => self.status = "Opened the portable UCI package folder.".into(),
                            Err(error) => {
                                self.status = format!("Could not open package folder: {error}");
                            }
                        }
                    }
                    if ui.button("Open review in FishEye").clicked() {
                        self.open_record_in_fisheye(&record);
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
            if self.recent_install.as_deref() == Some(&install_id) {
                self.recent_install = None;
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
            if let Some(progress) = self.install_progress {
                let available_width = ui.available_width();
                ui.add(
                    egui::ProgressBar::new(progress_fraction(progress))
                        .desired_width(available_width)
                        .text(progress_text(progress)),
                );
            }
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

pub fn show_startup_error(error: &impl std::fmt::Display) {
    let message = format!("UCI Grabber could not start:\n\n{error}");
    let _ = rfd::MessageDialog::new()
        .set_title("UCI Grabber")
        .set_description(&message)
        .set_level(rfd::MessageLevel::Error)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
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

fn pick_fisheye_executable() -> Option<std::path::PathBuf> {
    let dialog = rfd::FileDialog::new().set_title("Locate the FishEye application");
    #[cfg(target_os = "windows")]
    let dialog = dialog.add_filter("FishEye executable", &["exe"]);
    dialog.pick_file()
}

fn model_guidance(model: &Model) -> Option<&'static str> {
    let description = model.description.to_ascii_lowercase();
    if description.contains("balanced") || description.contains("general use") {
        Some("Recommended")
    } else if description.contains("smallest") || description.contains("fastest") {
        Some("Smallest & fastest")
    } else if description.contains("largest") {
        Some("Largest")
    } else {
        None
    }
}

fn hugging_face_model_card_url(download: &str) -> Option<String> {
    let prefix = download.strip_prefix("https://huggingface.co/")?;
    let (repository, revision_and_file) = prefix.split_once("/resolve/")?;
    if repository.split('/').count() != 2 {
        return None;
    }
    let (revision, filename) = revision_and_file.split_once('/')?;
    if revision.len() != 40
        || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        || filename.is_empty()
        || filename.contains('/')
    {
        return None;
    }
    Some(format!(
        "https://huggingface.co/{repository}/blob/{revision}/README.md"
    ))
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format_tenths(bytes, GIB, "GiB")
    } else if bytes >= MIB {
        format_tenths(bytes, MIB, "MiB")
    } else if bytes >= KIB {
        format_tenths(bytes, KIB, "KiB")
    } else {
        format!("{bytes} B")
    }
}

fn format_tenths(bytes: u64, unit: u64, suffix: &str) -> String {
    let tenths = bytes
        .saturating_mul(10)
        .saturating_add(unit / 2)
        .checked_div(unit)
        .unwrap_or_default();
    format!("{}.{} {suffix}", tenths / 10, tenths % 10)
}

fn progress_fraction(progress: InstallProgress) -> f32 {
    if progress.total_bytes == 0 {
        return 0.0;
    }
    let permille = progress
        .completed_bytes
        .saturating_mul(1000)
        .checked_div(progress.total_bytes)
        .unwrap_or_default()
        .min(1000);
    f32::from(u16::try_from(permille).unwrap_or(1000)) / 1000.0
}

fn progress_text(progress: InstallProgress) -> String {
    let phase = match (progress.phase, progress.artifact_kind) {
        (InstallPhase::Downloading, Some(ArtifactKind::Runtime)) => "Downloading runtime",
        (InstallPhase::Downloading, Some(ArtifactKind::Model)) => "Downloading model",
        (InstallPhase::Downloading, Some(ArtifactKind::Other) | None) => "Downloading package",
        (InstallPhase::Verifying, _) => "Verifying checksum",
        (InstallPhase::Extracting, _) => "Preparing portable package",
        (InstallPhase::ValidatingUci, _) => "Testing UCI compatibility",
        (InstallPhase::Activating, _) => "Saving portable package",
        (InstallPhase::Ready, _) => "UCI engine ready",
    };
    if progress.phase == InstallPhase::Downloading {
        format!(
            "{phase} • overall {} / {}",
            format_bytes(progress.completed_bytes),
            format_bytes(progress.total_bytes)
        )
    } else {
        phase.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(description: &str) -> Model {
        Model {
            id: "fixture".into(),
            name: "Fixture".into(),
            description: description.into(),
            packages: Vec::new(),
        }
    }

    #[test]
    fn maia_descriptions_produce_clear_choice_guidance() {
        assert_eq!(
            model_guidance(&model("The balanced checkpoint for general use.")),
            Some("Recommended")
        );
        assert_eq!(
            model_guidance(&model("The smallest and fastest checkpoint.")),
            Some("Smallest & fastest")
        );
        assert_eq!(
            model_guidance(&model("The largest checkpoint.")),
            Some("Largest")
        );
        assert_eq!(model_guidance(&model("A specialist checkpoint.")), None);
    }

    #[test]
    fn byte_sizes_are_human_readable() {
        assert_eq!(format_bytes(20_968_049), "20.0 MiB");
        assert_eq!(format_bytes(91_799_307), "87.5 MiB");
        assert_eq!(format_bytes(315_651_851), "301.0 MiB");
    }

    #[test]
    fn pinned_checkpoint_links_expose_the_matching_model_card() {
        assert_eq!(
            hugging_face_model_card_url(
                "https://huggingface.co/UofTCSSLab/Maia3-5M/resolve/\
b6559de2398d7140b985f28fd2c19fb5e47ddabe/maia3-5m.pt"
            )
            .as_deref(),
            Some(
                "https://huggingface.co/UofTCSSLab/Maia3-5M/blob/\
b6559de2398d7140b985f28fd2c19fb5e47ddabe/README.md"
            )
        );
        assert!(hugging_face_model_card_url("https://example.test/model.pt").is_none());
    }

    #[test]
    fn install_progress_is_bounded_and_descriptive() {
        let progress = InstallProgress {
            phase: InstallPhase::Downloading,
            artifact_kind: Some(ArtifactKind::Model),
            completed_bytes: 25,
            total_bytes: 100,
        };
        assert!((progress_fraction(progress) - 0.25).abs() < f32::EPSILON);
        assert_eq!(
            progress_text(progress),
            "Downloading model • overall 25 B / 100 B"
        );
    }
}
