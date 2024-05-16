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
    },
    Restore {
        name: String,
        destination: String
    },
    List {},
    Validate {},
    RebuildCache {},
}

#[tokio::main]
async fn main() {
    let logger = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .build();

    let multi_progress = MultiProgress::new();

    LogWrapper::new(multi_progress.clone(), logger)
        .try_init()
        .unwrap();

    console_subscriber::init();
    let cli = Cli::parse();
    let content = std::fs::read_to_string("backup.toml").unwrap();
    let config: BackupConfig = toml::from_str(&content).unwrap();


    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // invoke the default handler and exit the process
        orig_hook(panic_info);
        println!("Exiting due to panic");
        std::process::exit(1);
    }));

    match &cli.command {
        Commands::Backup { force_hash, dry_run } => {
            backup::run_backup(config, backup::generate_name(), multi_progress, !!force_hash, !!dry_run).await
        }
        Commands::Restore { name, destination } => {
            restore::restore_backup(
                PathBuf::from(destination),
                name, 
                config.stores.get(0).unwrap(),
                config.encrypting_key_file,
                &config.hmac_secret,
                &config.signing_key_file,
            ).await
        }
        Commands::List {} => {
            list::list_backups(config.stores.get(0).unwrap()).await
        }
        Commands::Validate {} => { println!("Todo") }
        Commands::RebuildCache {} => {
            rebuild_cache::rebuild_cache(config).await
        }
    }
// validate
// rebuild-cache
}
