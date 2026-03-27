# backup-tool

A Rust CLI tool that incrementally backs up a directory to [OpenStack Swift](https://wiki.openstack.org/wiki/Swift) object storage. Files are content-hashed (HMAC-SHA-512), PGP-encrypted, and uploaded. A local SQLite cache avoids re-uploading files that haven't changed. File metadata (names, mtimes, permissions) is stored in a separate encrypted SQLite file which is also uploaded.

## Features

- **Incremental** — HMAC hash of each file's contents is cached; unchanged files are skipped
- **Deduplicating** — Files with identical contents are only stored once
- **Encrypted** — all data and metadata is PGP-encrypted before upload; optionally signed
- **Multi-store** — the same backup can be written to multiple Swift targets simultaneously
- **Re-buildable cache** — `rebuild-cache` repopulates the local upload cache from Swift if it is lost

## Building

### Docker (recommended)

```bash
docker build -t backup-tool .
```

The image uses a multi-stage build: Rust 1.84 compiles the binary, which is then copied into a minimal Ubuntu 22.04 runtime image.

### Native

Native dependencies: `clang`, `llvm`, `pkg-config`, `nettle-dev` (required by sequoia-openpgp).

```bash
apt-get install -y clang llvm pkg-config nettle-dev  # Debian/Ubuntu
cargo build --release
```

## Configuration

backup-tool reads a TOML config file (`backup.toml` by default; override with `--config`).

```toml
# Directory to back up
source = "/home/user/documents"

# Local working directories (created automatically)
data_cache     = "/var/cache/backup-tool/cache.db"
metadata_cache = "/var/cache/backup-tool"

# HMAC-SHA-512 secret used as the content-hash key.
# Changing this invalidates the local cache and backed-up data objects.
hmac_secret = "change-me-to-a-long-random-string"

# PGP public key used to encrypt every uploaded object
encrypting_key_file = "/etc/backup-tool/encrypt.pub.asc"

# Optional: PGP private key used to sign the metadata file
# signing_key_file = "/etc/backup-tool/signing.key.asc"

[[stores]]
id                 = 1
data_container     = "my-backups-data"
metadata_container = "my-backups-metadata"
data_prefix        = "data/"
metadata_prefix    = "meta/"

# OpenStack credentials for this store.
# If omitted, osauth falls back to OS_* environment variables / clouds.yaml.
# [stores.data_cloud_config]
# auth_type = "v3applicationcredential"
# auth_url  = "https://identity.example.com/v3"
# ...
```

Multiple `[[stores]]` sections are supported; backups are written to all of them in parallel.

### OpenStack authentication

Authentication is delegated to [osauth](https://github.com/dtantsur/rust-osauth). Each store can embed a `data_cloud_config` / `metadata_cloud_config` block, or rely on the standard OpenStack environment variables (`OS_AUTH_URL`, `OS_APPLICATION_CREDENTIAL_ID`, etc.) or a `clouds.yaml` file.

## Usage

```
backup-tool [--config <path>] <command>
```

| Command | Description |
|---|---|
| `backup` | Run an incremental backup |
| `restore <name> <destination>` | Restore a named backup to a local directory |
| `list` | List all available backups in the store |
| `rebuild-cache` | Rebuild the local upload cache from Swift |

### `backup`

```bash
backup-tool backup
backup-tool backup --force-hash   # re-hash every file, ignoring the local cache
backup-tool backup --dry-run      # walk and hash files without uploading
```

Each backup is stored under a timestamped name (e.g. `2026-03-27T14-05-32`). The backup pipeline is:

1. Walk `source`, computing a filesystem-metadata hash (path + size + mtime) per file
2. For cache misses, compute the HMAC-SHA-512 content hash (rayon thread pool)
3. PGP-encrypt any files not yet in Swift (rayon thread pool)
4. Upload encrypted blobs and record them in the local SQLite cache
5. Write a metadata SQLite file, encrypt it, and upload it as `<metadata_prefix><name>.metadata`

### `restore`

```bash
backup-tool restore 2026-03-27T14-05-32 /mnt/restore
```

The destination directory must not already exist. The tool downloads and decrypts the metadata file, then streams file entries and restores each one. Content hashes are verified after decryption. Available disk space is checked before starting.

### `list`

```bash
backup-tool list
```

Prints the names of all backups found in the metadata container of the first configured store.

### `rebuild-cache`

```bash
backup-tool rebuild-cache
```

Clears and repopulates the `uploaded_objects` table in the local cache by listing all objects in each store's data container. Useful after losing or moving `cache.db`.

## Development

### Running locally

```bash
cargo run -- --config backup.toml list
```

### Async task debugging (tokio-console)

```bash
RUSTFLAGS="--cfg tokio_unstable" cargo run --features console -- backup
```

### Logging

Set `RUST_LOG` to control verbosity:

```bash
RUST_LOG=trace backup-tool backup
RUST_LOG=backup_tool::upload_worker=debug backup-tool backup
```
