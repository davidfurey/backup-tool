# Metadata Database Format

This document describes the metadata SQLite database used by `backup-tool` in enough detail to write an independent restore implementation.

---

## Overview

Each backup produces a single SQLite database file. That file is OpenPGP-encrypted and uploaded to the **metadata** Swift container as:

```
{metadata_prefix}{backup_name}.metadata
```

For example, with `metadata_prefix = "meta/"` and backup name `backup-2024-01-15T10:30:00Z-aBcD`:

```
meta/backup-2024-01-15T10:30:00Z-aBcD.metadata
```

The backup name format is `backup-{RFC3339 UTC timestamp with second precision}-{4 random alphanumeric chars}`.

To restore you must:
1. Download the `.metadata` object from the metadata container.
2. Decrypt it with the recipient's OpenPGP private key.
3. Open the resulting SQLite file and read its tables as described below.
4. For each file entry, download and decrypt the corresponding data object from the data container.

---

## Encryption

Both the metadata file and every data object are encrypted with **OpenPGP** (via [Sequoia](https://sequoia-pgp.org/)).

- **Encryption**: public-key encryption for a single recipient; optionally signed.
- **Compression**: `Uncompressed` (no compression algorithm is applied at the OpenPGP layer; data objects may already be compressed depending on content).
- **Decryption**: requires the recipient's secret key (the private counterpart of the `encrypting_key_file` configured at backup time). If the backup was created with a `signing_key_file`, the signature can optionally be verified.

Standard OpenPGP libraries (e.g. GnuPG, Sequoia, OpenPGP.js) can decrypt these files given the private key.

---

## SQLite Database Schema

The decrypted metadata file is a standard SQLite 3 database (WAL journal mode is **not** used for metadata files — they are written with `DELETE` journal mode and are read-only after creation).

### Table: `metadata`

Key/value pairs for backup-level properties.

```sql
CREATE TABLE metadata (
    key   TEXT,
    value TEXT
);
```

| key       | value description |
|-----------|-------------------|
| `version` | Schema version. Currently always `"0"`. Reject the file if this is not `"0"`. |
| `size`    | Total unencrypted size of all file content in bytes, as a decimal integer string. Used as a pre-flight disk-space check before restoring. |

Additional keys may be present in future schema versions.

### Table: `files`

One row per filesystem entry recorded in the backup, in the order they were visited by a depth-first directory walk (i.e. `walkdir` default order, roughly lexicographic within each directory level).

```sql
CREATE TABLE files (
    id          INTEGER PRIMARY KEY,
    name        TEXT,
    mtime       INTEGER,
    mode        INTEGER,
    ttype       STRING,
    destination STRING NULL,
    data_hash   STRING NULL
);
```

#### Column Reference

| Column        | Type            | Nullable | Description |
|---------------|-----------------|----------|-------------|
| `id`          | INTEGER         | No       | Insertion-order primary key (1-based, sequential). No semantic meaning beyond ordering. |
| `name`        | TEXT            | No       | Path of the entry **relative to the backup source root**, using the native path separator. For example `docs/reports/q1.txt` or `media/images/photo.jpg`. The source root directory itself is never included as a row. |
| `mtime`       | INTEGER         | No       | Last-modified time as a Unix timestamp (seconds since 1970-01-01 00:00:00 UTC). Must be applied after restoring the file. |
| `mode`        | INTEGER         | No       | Unix permission bits as a 32-bit integer (same value as `st_mode` from `stat(2)`, masked to the permission bits). Applied via `chmod`/`set_permissions` after writing the file. **Not applied to symlinks** (symlink permissions are always `rwxrwxrwx` on Linux and are not meaningful). |
| `ttype`       | STRING          | No       | Entry type: one of `FILE`, `SYMLINK`, or `DIRECTORY`. See [Entry Types](#entry-types) below. |
| `destination` | STRING          | Yes      | For `SYMLINK` entries: the symlink target exactly as it was stored (may be relative or absolute). `NULL` for `FILE` and `DIRECTORY` entries. |
| `data_hash`   | STRING          | Yes      | For `FILE` entries: the hex-encoded HMAC-SHA512 content hash (see [Data Hash](#data-hash)). This value is also the object key suffix in the data container. `NULL` for `SYMLINK` and `DIRECTORY` entries, and for empty files that produce no data object. |

---

## Entry Types

### `FILE`

A regular file. To restore:
1. Retrieve `data_hash` from the row.
2. Download the object at `{data_prefix}{data_hash}` from the data Swift container.
3. Decrypt the downloaded object (OpenPGP, same key as the metadata file).
4. Verify the decrypted content by recomputing `HMAC-SHA512(content, hmac_secret)` and comparing (as case-insensitive hex) with `data_hash`. Abort if they do not match.
5. Write the decrypted content to `{destination_root}/{name}`, creating parent directories as needed.
6. Set the file's modification time to `mtime` (Unix seconds).
7. Set the file's permissions to `mode`.

If two or more `FILE` rows share the same `data_hash` the data object is a **deduplication target** — only one object exists in the data container and all matching rows restore to identical content.

### `SYMLINK`

A symbolic link. To restore:
1. Create a symlink at `{destination_root}/{name}` pointing to `destination`.
2. Set the symlink's modification time to `mtime` using `lutimes`/`lchown` equivalents (do NOT follow the symlink when setting times).
3. Do **not** set permissions on the symlink itself.

### `DIRECTORY`

A directory. To restore:
1. Create the directory at `{destination_root}/{name}` (including any missing parents).
2. Set permissions to `mode`.
3. **Do not** set `mtime` until all entries beneath this directory have been restored. Directory mtimes are reset each time a child is created. Apply directory mtimes in a **second pass**, processing entries in **reverse lexicographic order** (deepest paths first) so that setting a child's mtime does not disturb its parent's mtime.

---

## Data Hash

The `data_hash` value is computed as:

```
HMAC-SHA512(file_content, hmac_secret)
```

where:
- `hmac_secret` is the value of `hmac_secret` from `backup.toml` (a UTF-8 string).
- `file_content` is the raw bytes of the original (pre-encryption) file.
- The result is formatted as an **uppercase hex string** (128 hex characters).

This value serves both as a content integrity check and as the object key in the data Swift container:

```
{data_prefix}{data_hash}
```

For example, with `data_prefix = "data/"`:

```
data/A3F2...C901
```

---

## Data Container Object Layout

Each unique file is stored as a single encrypted object. Multiple backup runs that encounter the same content share one object (deduplication is by `data_hash`).

```
{data_prefix}{data_hash}          ← encrypted file content (OpenPGP)
```

There are no subdirectories or additional metadata stored alongside the object.

---

## Restore Algorithm Summary

```
1.  Authenticate to OpenStack Swift (OS_* environment variables or cloud config).

2.  Download: {metadata_container}/{metadata_prefix}{backup_name}.metadata

3.  Decrypt the downloaded file with the recipient's OpenPGP private key.
    → result: metadata.sqlite (SQLite 3 database)

4.  Open metadata.sqlite.

5.  Read and validate:
      SELECT value FROM metadata WHERE key = 'version';
      → must be "0"

6.  Read total size for disk space pre-check:
      SELECT value FROM metadata WHERE key = 'size';

7.  Stream all entries in insertion order:
      SELECT id, name, mtime, mode, ttype, destination, data_hash
      FROM files
      ORDER BY id ASC;

8.  First pass — create filesystem entries:
    For each row:
      DIRECTORY → mkdir -p, chmod mode
      SYMLINK   → symlink(destination, path), lutimes(path, mtime)
      FILE      → download+decrypt data object, verify HMAC, write file,
                  chmod mode, set mtime

9.  Second pass — fix directory mtimes:
    Collect all DIRECTORY rows, sort by name DESC (deepest first),
    then set mtime on each directory path.

10. Clean up temporary files.
```

---

## Configuration Reference

The fields relevant to understanding where objects are stored come from `backup.toml`:

| Field                  | Description |
|------------------------|-------------|
| `hmac_secret`          | Secret used in HMAC-SHA512 to compute `data_hash`. Required to verify restored file integrity. |
| `encrypting_key_file`  | Path to the PGP public key used for encryption. The corresponding private key is needed for decryption/restore. |
| `signing_key_file`     | *(Optional)* Path to a PGP key used to sign data at backup time. Pass the corresponding public key during decryption to verify signatures. |
| `stores[].data_container`     | Swift container name for data objects. |
| `stores[].metadata_container` | Swift container name for metadata objects. |
| `stores[].data_prefix`        | String prepended to `data_hash` to form the Swift object key for data. |
| `stores[].metadata_prefix`    | String prepended to `{backup_name}.metadata` to form the Swift object key for the metadata file. |

OpenStack credentials are read from `OS_*` environment variables (standard OpenStack client variables: `OS_AUTH_URL`, `OS_USERNAME`, `OS_PASSWORD`, `OS_PROJECT_NAME`, `OS_USER_DOMAIN_NAME`, `OS_PROJECT_DOMAIN_NAME`, `OS_IDENTITY_API_VERSION`), or from a `data_cloud_config` / `metadata_cloud_config` block embedded in the store configuration.
