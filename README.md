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
# Generate one with: openssl rand -hex 32
hmac_secret = "change-me-to-a-long-random-string"

# PGP public key used to encrypt every uploaded object
encrypting_key_file = "/etc/backup-tool/encrypt.pub.asc"

# Optional: PGP private key used to sign the metadata file
# signing_key_file = "/etc/backup-tool/signing.key.asc"

[[stores]]
id                 = 1
container          = "my-backups"
data_prefix        = "data/"
metadata_prefix    = "meta/"

# Whether to upload data/metadata objects to this store (both default to true).
# Set to false to create a metadata-only or data-only mirror store.
# upload_data     = true
# upload_metadata = true

# OpenStack credentials for this store.
# If omitted, osauth falls back to OS_* environment variables / clouds.yaml.
# [stores.cloud_config]
# auth_type = "v3applicationcredential"
# auth_url  = "https://identity.example.com/v3"
# ...

# Alternatively, back a store with a local directory instead of Swift.
# When local_path is set, container and cloud_config are ignored.
# [[stores]]
# id              = 2
# local_path      = "/mnt/backup-mirror"
# data_prefix     = "data/"
# metadata_prefix = "meta/"
```

Multiple `[[stores]]` sections are supported; backups are written to all of them in parallel. The `upload_data` and `upload_metadata` flags (both default `true`) let you create partial-mirror stores — for example a store that receives metadata only (useful for fast `list`/`validate` without storing data twice) or data only.

Stores backed by a local directory use `local_path` instead of `container`. No OpenStack credentials are needed; objects are stored as plain files under the given directory using the same key structure (`<prefix><hash>` for data, `<prefix><name>.metadata` for metadata).

### Creating keys

Use separate keys for encryption and signing so you can enforce least privilege and reduce blast radius. This lets backup hosts encrypt and sign new backups without holding decryption material, while restore-capable systems can keep decryption keys isolated.

1. Generate an encryption keypair (used to encrypt backup data and metadata):

```bash
sq key generate \
	--userid "<test@example.com>" \
	--cannot-authenticate \
	--cannot-sign \
	--expiration never \
	--output encrypt.pgp \
	--rev-cert encrypt.rev \
	--shared-key \
	--without-password
```

2. Extract the public certificate for backup-only hosts (encrypt but cannot decrypt):

```bash
sq key extract-cert --output encrypt.cert encrypt.pgp
```

3. Generate a signing keypair for metadata signatures:

```bash
sq key generate \
	--userid "<test@example.com>" \
	--cannot-authenticate \
	--cannot-encrypt \
	--expiration never \
	--output sign.pgp \
	--rev-cert sign.rev \
	--shared-key \
	--without-password
```

Recommended config mapping:

- `encrypting_key_file`: use `encrypt.cert` on backup hosts (or `encrypt.pgp` where decryption is also needed).
- `signing_key_file`: use `sign.pgp` on hosts that create backups.

### OpenStack authentication

Authentication is delegated to [osauth](https://github.com/dtantsur/rust-osauth). This only applies to Swift-backed stores (those without `local_path`). Each store can embed a `cloud_config` block, or rely on the standard OpenStack environment variables (`OS_AUTH_URL`, `OS_APPLICATION_CREDENTIAL_ID`, etc.) or a `clouds.yaml` file.

## Usage

```
backup-tool [--config <path>] <command>
```

| Command | Description |
|---|---|
| `backup` | Run an incremental backup |
| `restore <name> <destination>` | Restore a named backup to a local directory |
| `list` | List available backups across all (or selected) stores |
| `validate <name>` | Verify all data objects for a backup exist in every (or selected) store |
| `rebuild-cache` | Rebuild the local upload cache from Swift (all or selected stores) |

### `backup`

```bash
backup-tool backup
backup-tool backup --force-hash        # re-hash every file, ignoring the local cache
backup-tool backup --dry-run           # walk and hash files without uploading
backup-tool backup --limit 1,2        # upload only to stores with id 1 and 2
```

Each backup is stored under a timestamped name (e.g. `backup-2026-03-27T14:05:32Z-a1B2`). The backup pipeline is:

1. Walk `source`, computing a filesystem-metadata hash (path + size + mtime) per file
2. For cache misses, compute the HMAC-SHA-512 content hash (rayon thread pool)
3. PGP-encrypt any files not yet in Swift (rayon thread pool)
4. Upload encrypted blobs and record them in the local SQLite cache
5. Write a metadata SQLite file, encrypt it, and upload it as `<metadata_prefix><name>.metadata`

### `restore`

```bash
backup-tool restore backup-2026-03-27T14:05:32Z-a1B2 /mnt/restore
backup-tool restore backup-2026-03-27T14:05:32Z-a1B2 /mnt/restore --store-id 2
backup-tool restore backup-2026-03-27T14:05:32Z-a1B2 /mnt/restore --store-id 2 --metadata-store-id 3
```

`--store-id` selects the store to fetch data objects from (defaults to `1`). `--metadata-store-id` selects the store to fetch the metadata file from; if omitted it defaults to `--store-id`. This lets you restore data objects from one store while reading the metadata file from another (e.g. a store that only holds metadata).

The destination directory must not already exist. The tool downloads and decrypts the metadata file, then streams file entries and restores each one. Content hashes are verified after decryption. Available disk space is checked before starting.

### `list`

```bash
backup-tool list                  # all configured stores
backup-tool list --limit 1,2     # only stores 1 and 2
```

Prints the names of all backups found across the queried stores. Backups present in every queried store are printed as-is; backups missing from one or more stores are annotated:

```
backup-2026-03-27T14:05:32Z-a1B2
backup-2026-03-25T09:00:00Z-x9Y2 [missing from store(s): 2]
```

`--limit` accepts a comma-separated list (`--limit 1,2`) or repeated flags (`--limit 1 --limit 2`).

### `validate`

```bash
backup-tool validate backup-2026-03-27T14:05:32Z-a1B2
backup-tool validate backup-2026-03-27T14:05:32Z-a1B2 --limit 1,2  # check only stores 1 and 2
```

Checks that every file stored in the named backup is present in every queried store, without downloading or decrypting any data. Before checking file objects, the tool downloads and decrypts the metadata file from **all** queried stores and verifies the SHA-256 of the decrypted content is identical across them, aborting if they differ.

1. Downloads and decrypts the backup's metadata file
2. Authenticates to each store once up-front
3. Issues a HEAD request per `(file, store)` pair, up to 16 concurrently
4. Logs each missing object at `error` level with its store ID, truncated hash, and filename
5. Prints a final pass/fail summary via an indicatif progress bar
6. Exits with status 1 if any objects are missing or checks fail (suitable for CI)

A passing validation prints:

```
[Validate]  ✓ [0:00:05] 42 files checked — passed (2 store(s))
```

A failing one — `MISSING` lines come from the error logger (controlled by `RUST_LOG`), followed by the final progress line:

```
ERROR MISSING  store=2  hash=a3f1b2c4d5e6f708  file=docs/reports/report.pdf
[Validate]  ✓ [0:00:05] 84 files checked — FAILED: 1/84 missing
```

Directories and symlinks are not checked — they have no data object in Swift.

### `rebuild-cache`

```bash
backup-tool rebuild-cache                  # all configured stores
backup-tool rebuild-cache --limit 1,2     # only stores 1 and 2
```

Clears and repopulates the `uploaded_objects` table in the local cache by listing all objects in each store's data container. When `--limit` is given, only the rows for the specified stores are cleared and then repopulated; rows for other stores are left untouched. Useful after losing or moving `cache.db`.

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
