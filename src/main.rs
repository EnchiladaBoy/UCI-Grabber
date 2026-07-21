use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use clap::{Args, Parser, Subcommand};

use uci_grabber::app;
use uci_grabber::catalog::{CatalogCache, default_client, preferred_catalog};
use uci_grabber::handoff;
use uci_grabber::install::{InstallOptions, Installer};
use uci_grabber::recipes::CustomRecipeStore;
use uci_grabber::registry::{InstallSource, Integrity, RegistryStore};
use uci_grabber::schema::Recipe;
use uci_grabber::{Error, Result};

#[derive(Debug, Parser)]
#[command(name = "uci-grabber", version, about)]
struct Cli {
    /// Override the application data directory (useful for portable/test setups).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List available curated and imported UCI packages.
    List(ListArgs),
    /// Install one model from a recipe ID.
    Install(InstallArgs),
    /// Validate and store an unreviewed local or HTTPS recipe.
    Import(ImportArgs),
    /// Show installed generations and executable integrity.
    Status(StatusArgs),
    /// Open an installed executable in `FishEye`'s review dialog.
    OpenInFisheye(OpenArgs),
    /// Explicitly remove an immutable installed generation.
    Remove(RemoveArgs),
}

#[derive(Debug, Args)]
struct ListArgs {
    /// Re-download and verify this app release's immutable catalog first.
    #[arg(long)]
    refresh: bool,
}

#[derive(Debug, Args)]
struct InstallArgs {
    recipe_id: String,
    #[arg(long)]
    model: String,
    /// Select an exact custom recipe version when more than one is imported.
    #[arg(long)]
    version: Option<String>,
    /// Approve running unreviewed native code with your account permissions and no OS sandbox.
    #[arg(long)]
    approve_unreviewed: bool,
}

#[derive(Debug, Args)]
struct ImportArgs {
    /// A local JSON file or HTTPS URL.
    source: String,
}

#[derive(Debug, Args)]
struct StatusArgs {
    /// Clean interrupted staging and repair records for activated generations.
    #[arg(long)]
    repair: bool,
}

#[derive(Debug, Args)]
struct OpenArgs {
    install_id: String,
    #[arg(long)]
    fisheye: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct RemoveArgs {
    install_id: String,
    /// Required to acknowledge deletion of this generation.
    #[arg(long)]
    confirm: bool,
}

fn main() {
    let graphical_launch = std::env::args_os().len() == 1;
    if let Err(error) = run() {
        eprintln!("UCI Grabber: {error}");
        if graphical_launch {
            app::show_startup_error(&error);
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.command.is_none() {
        return if cli.data_dir.is_none() {
            app::run_gui()
        } else {
            Err(Error::Other(
                "--data-dir requires a CLI subcommand and is not silently applied to the GUI"
                    .into(),
            ))
        };
    }
    let store = match cli.data_dir {
        Some(path) => RegistryStore::new(path),
        None => RegistryStore::open_default()?,
    };
    let installer = Installer::new(
        store.clone(),
        std::sync::Arc::new(uci_grabber::download::HttpDownloader::default()),
    );
    let custom_store = CustomRecipeStore::new(store.data_root().join("custom-recipes"));
    match cli.command {
        None => unreachable!("the no-subcommand case returned before store construction"),
        Some(Command::List(arguments)) => list(&store, &custom_store, &arguments),
        Some(Command::Install(arguments)) => install(&store, &custom_store, &installer, &arguments),
        Some(Command::Import(arguments)) => {
            let recipe = custom_store.import(&arguments.source)?;
            println!(
                "Imported unreviewed recipe {} {} ({} model{}).",
                recipe.name,
                recipe.version,
                recipe.models.len(),
                if recipe.models.len() == 1 { "" } else { "s" }
            );
            println!("Review it before installing; import never executes package content.");
            Ok(())
        }
        Some(Command::Status(arguments)) => status(&installer, &arguments),
        Some(Command::OpenInFisheye(arguments)) => open(&store, &arguments),
        Some(Command::Remove(arguments)) => remove(&store, &arguments),
    }
}

fn list(store: &RegistryStore, custom: &CustomRecipeStore, arguments: &ListArgs) -> Result<()> {
    let client = default_client(CatalogCache::new(store.cache_dir()))?;
    let catalog = if arguments.refresh {
        client.refresh()?
    } else {
        preferred_catalog(&client)?
    };
    println!("Curated (release-verified):");
    print_recipes(&catalog.catalog.recipes);
    println!("Unreviewed custom:");
    print_recipes(&custom.load_all()?);
    Ok(())
}

fn print_recipes(recipes: &[Recipe]) {
    if recipes.is_empty() {
        println!("  (none)");
        return;
    }
    for recipe in recipes {
        let models = recipe
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "  {}  {} {}  models: {}",
            recipe.id, recipe.name, recipe.version, models
        );
    }
}

fn install(
    store: &RegistryStore,
    custom: &CustomRecipeStore,
    installer: &Installer,
    arguments: &InstallArgs,
) -> Result<()> {
    let client = default_client(CatalogCache::new(store.cache_dir()))?;
    let catalog = preferred_catalog(&client)?;
    let curated = catalog
        .catalog
        .recipes
        .into_iter()
        .find(|recipe| recipe.id == arguments.recipe_id);
    let (recipe, source) = if let Some(recipe) = curated {
        (recipe, InstallSource::Curated)
    } else {
        let mut matches = custom
            .load_all()?
            .into_iter()
            .filter(|recipe| recipe.id == arguments.recipe_id)
            .filter(|recipe| {
                arguments
                    .version
                    .as_ref()
                    .is_none_or(|version| &recipe.version == version)
            })
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(Error::InvalidRecipe(format!(
                "multiple versions of `{}` are imported; select one with --version",
                arguments.recipe_id
            )));
        }
        let recipe = matches.pop().ok_or_else(|| {
            Error::InvalidRecipe(format!("unknown recipe `{}`", arguments.recipe_id))
        })?;
        (recipe, InstallSource::UnreviewedRecipe)
    };
    let options = InstallOptions {
        source,
        approve_unreviewed: arguments.approve_unreviewed,
        ..InstallOptions::default()
    };
    let record = installer.install(&recipe, &arguments.model, &options, &AtomicBool::new(false))?;
    println!("Installed and UCI-validated {}.", record.name);
    println!("{}", record.executable.display());
    Ok(())
}

fn status(installer: &Installer, arguments: &StatusArgs) -> Result<()> {
    if arguments.repair {
        let report = installer.recover()?;
        println!(
            "Recovery: cleaned {} staging director{}, repaired {} record{}.",
            report.cleaned_staging,
            if report.cleaned_staging == 1 {
                "y"
            } else {
                "ies"
            },
            report.repaired_records,
            if report.repaired_records == 1 {
                ""
            } else {
                "s"
            }
        );
    }
    let registry = installer.store().load()?;
    if registry.installs.is_empty() {
        println!("No installed UCI packages.");
        return Ok(());
    }
    for record in registry.installs {
        let integrity = match RegistryStore::integrity(&record)? {
            Integrity::Verified => "verified".to_owned(),
            Integrity::Missing => "missing".to_owned(),
            Integrity::Changed { actual, .. } => format!("changed ({actual})"),
        };
        println!(
            "{}  {} {}  {}  {}\n  {}",
            record.install_id,
            record.name,
            record.version,
            record.source,
            integrity,
            record.executable.display()
        );
    }
    Ok(())
}

fn open(store: &RegistryStore, arguments: &OpenArgs) -> Result<()> {
    let registry = store.load()?;
    let record = registry
        .installs
        .iter()
        .find(|record| record.install_id == arguments.install_id)
        .ok_or_else(|| Error::Other(format!("unknown install `{}`", arguments.install_id)))?;
    match RegistryStore::integrity(record)? {
        Integrity::Verified => {}
        Integrity::Missing | Integrity::Changed { .. } => {
            return Err(Error::Other(
                "refusing to hand off a missing or changed executable".into(),
            ));
        }
    }
    handoff::open_in_fisheye(&record.executable, arguments.fisheye.as_deref())?;
    println!("Opened FishEye external engine manager.");
    println!("Fallback path: {}", record.executable.display());
    Ok(())
}

fn remove(store: &RegistryStore, arguments: &RemoveArgs) -> Result<()> {
    if !arguments.confirm {
        return Err(Error::Other(
            "removal requires --confirm; this deletes exactly one immutable generation".into(),
        ));
    }
    if store.remove(&arguments.install_id)? {
        println!("Removed {}.", arguments.install_id);
    } else {
        println!("Install {} was not present.", arguments.install_id);
    }
    Ok(())
}
