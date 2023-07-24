pub mod encryption;
pub mod decryption;
pub mod sharding;
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

use std::path::PathBuf;

use config::BackupConfig;

use clap::{Parser, Subcommand};

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
    },
    Restore {
        name: String
    },
    List {},
    Validate {},
    RebuildCache {},
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let content = std::fs::read_to_string("backup.toml").unwrap();
    let config: BackupConfig = toml::from_str(&content).unwrap();

    match &cli.command {
        Commands::Backup {} => {
            backup::run_backup(config)
        }
        Commands::Restore { name } => {
            restore::restore_backup(
                PathBuf::from("/tmp/restore"),
                name, 
                config.stores.get(0).unwrap(),
                config.key_file
            ).await
        }
        Commands::List {} => {
            list::list_backups(config.stores.get(0).unwrap()).await
        }
        Commands::Validate {} => { println!("Todo") }
        Commands::RebuildCache {} => { println!("Todo") }
    }
// validate
// rebuild-cache
}
