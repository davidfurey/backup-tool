pub mod encryption;
pub mod decryption;
pub mod swift;
pub mod datastore;
pub mod metadata_file;
pub mod sqlite_cache;
pub mod hash;
pub mod filetype;
pub mod config;
pub mod upload_worker;
pub mod hash_worker;
pub mod backup;
pub mod restore;
pub mod list;
pub mod query;
pub mod rebuild_cache;
pub mod utils;

use std::path::PathBuf;

use config::BackupConfig;

use clap::{Parser, Subcommand};
use indicatif::MultiProgress;
use indicatif_log_bridge::LogWrapper;

extern crate serde;
#[macro_use]
extern crate serde_derive;

#[derive(Parser)]
struct Cli {
    #[arg(short, long, default_value = "backup.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Backup {
        #[arg(short, long, default_value_t = false)]
        force_hash: bool,
        #[arg(short, long, default_value_t = false)]
        dry_run: bool,
        /// Restrict to these store ids (comma-separated or repeated). Omit to use all stores.
        #[arg(short, long, value_delimiter = ',', num_args = 0..)]
        limit: Vec<i32>,
    },
    Restore {
        name: String,
        destination: String,
        /// Store to fetch data objects from.
        #[arg(short, long, default_value_t = 1)]
        store_id: i32,
        /// Store to fetch the metadata file from. Defaults to --store-id if not specified.
        #[arg(long)]
        metadata_store_id: Option<i32>,
    },
    List {
        /// Restrict to these store ids (comma-separated or repeated). Omit to use all stores.
        #[arg(short, long, value_delimiter = ',', num_args = 0..)]
        limit: Vec<i32>,
    },
    Validate {
        name: String,
        /// Restrict to these store ids (comma-separated or repeated). Omit to use all stores.
        #[arg(short, long, value_delimiter = ',', num_args = 0..)]
        limit: Vec<i32>,
    },
    RebuildCache {
        /// Restrict to these store ids (comma-separated or repeated). Omit to use all stores.
        #[arg(short, long, value_delimiter = ',', num_args = 0..)]
        limit: Vec<i32>,
    },
}

#[tokio::main]
async fn main() {
    let is_attended = console::user_attended_stderr();

    let logger = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(if is_attended { None } else { Some(Default::default()) })
        .format_level(!is_attended)
        .format_target(!is_attended)
        .build();

    let multi_progress = MultiProgress::new();

    LogWrapper::new(multi_progress.clone(), logger)
        .try_init()
        .unwrap();

    #[cfg(feature = "console")]
    console_subscriber::init();
    let cli = Cli::parse();
    let content = std::fs::read_to_string(&cli.config).unwrap();
    let config: BackupConfig = toml::from_str(&content).unwrap();


    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // invoke the default handler and exit the process
        orig_hook(panic_info);
        println!("Exiting due to panic");
        std::process::exit(1);
    }));

    let filter_stores = |stores: Vec<crate::datastore::DataStore>, limit: &Vec<i32>| {
        if limit.is_empty() {
            stores
        } else {
            stores.into_iter().filter(|s| limit.contains(&s.id)).collect()
        }
    };

    match &cli.command {
        Commands::Backup { force_hash, dry_run, limit } => {
            let mut filtered_config = config;
            filtered_config.stores = filter_stores(filtered_config.stores, limit);
            backup::run_backup(filtered_config, backup::generate_name(), multi_progress, !!force_hash, !!dry_run).await
        }
        Commands::Restore { name, destination, store_id, metadata_store_id } => {
            let data_store = config.stores.iter().find(|s| s.id == *store_id)
                .unwrap_or_else(|| panic!("No store with id {}", store_id));
            let meta_id = metadata_store_id.unwrap_or(*store_id);
            let metadata_store = config.stores.iter().find(|s| s.id == meta_id)
                .unwrap_or_else(|| panic!("No store with id {}", meta_id));
            restore::restore_backup(
                PathBuf::from(destination),
                name,
                metadata_store,
                data_store,
                config.encrypting_key_file,
                &config.hmac_secret,
                &config.signing_key_file,
                multi_progress,
            ).await
        }
        Commands::List { limit } => {
            let stores = filter_stores(config.stores, limit);
            list::list_backups(&stores).await
        }
        Commands::Validate { name, limit } => {
            let stores = filter_stores(config.stores, limit);
            let passed = restore::validate_backup(
                name,
                &stores,
                config.encrypting_key_file,
                &config.signing_key_file,
                multi_progress,
            ).await;
            if !passed {
                std::process::exit(1);
            }
        }
        Commands::RebuildCache { limit } => {
            rebuild_cache::rebuild_cache(config, limit).await
        }
    }
}
