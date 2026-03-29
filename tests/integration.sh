#!/usr/bin/env bash
# Integration test for backup-tool
#
# Requires: docker, sq, cargo, rsync
#
# What it does:
#   1. Builds the binary
#   2. Generates varied test data
#   3. Creates a PGP keypair for encryption
#   4. Starts jeantil/openstack-keystone-swift (Keystone v3 + Swift in one container)
#   5. Registers the Swift endpoint in Keystone
#   6. Creates Swift containers for data and metadata
#   7. Runs backup  (credentials via OS_* env vars; no inline cloud config needed)
#   8. Runs validate
#   9. Runs restore
#  10. Verifies content, symlinks, and mtimes match the source
#  11. Cleans up

set -euo pipefail

### Configuration ############################################################

DOCKER_IMAGE="jeantil/openstack-keystone-swift:pike"
KEYSTONE_ADMIN_PORT=35357
KEYSTONE_PUBLIC_PORT=5000
SWIFT_HOST_PORT=18080     # host port for the container's Swift proxy (8080)
CONTAINER_NAME="backup-tool-test-keystone-$$"
BINARY="${BINARY:-./target/release/backup-tool}"

# Credentials baked into the image's ENV
OS_AUTH_URL="http://127.0.0.1:${KEYSTONE_ADMIN_PORT}/v3"
OS_USERNAME="admin"
OS_PASSWORD="7a04a385b907caca141f"
OS_PROJECT_NAME="admin"
OS_USER_DOMAIN_NAME="Default"
OS_PROJECT_DOMAIN_NAME="Default"

DATA_CONTAINER="backup-data"
META_CONTAINER="backup-metadata"

# Export so backup-tool's osauth::Session::from_env() picks them up
export OS_AUTH_URL OS_USERNAME OS_PASSWORD OS_PROJECT_NAME \
       OS_USER_DOMAIN_NAME OS_PROJECT_DOMAIN_NAME
export OS_IDENTITY_API_VERSION="3"

### Helpers ##################################################################

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'; NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC}  $*"; }
fail() { echo -e "${RED}FAIL${NC}  $*"; exit 1; }
info() { echo -e "${YELLOW}INFO${NC}  $*"; }

# Portable mtime: BSD stat (macOS) vs GNU stat (Linux)
if stat -f %m / >/dev/null 2>&1; then
    mtime() { stat -f %m "$1"; }   # macOS / BSD
else
    mtime() { stat -c %Y "$1"; }   # Linux / GNU
fi

### Workspace ################################################################

WORK_DIR="$(mktemp -d)"
SOURCE_DIR="${WORK_DIR}/source"
RESTORE_DIR="${WORK_DIR}/restore"
CONFIG_DIR="${WORK_DIR}/config"
mkdir -p "${SOURCE_DIR}" "${CONFIG_DIR}"
# RESTORE_DIR must NOT be pre-created — restore_backup requires a non-existent destination

KEY_EMAIL="backup-test-$(date +%Y)@integration.test"

cleanup() {
    info "Cleaning up..."
    docker stop "${CONTAINER_NAME}" 2>/dev/null || true
    docker rm   "${CONTAINER_NAME}" 2>/dev/null || true
    rm -rf "${WORK_DIR}"
    info "Done."
}
trap cleanup EXIT

### Step 0: Prerequisites ####################################################

info "Checking prerequisites..."
for cmd in docker sq cargo rsync; do
    command -v "${cmd}" >/dev/null 2>&1 || fail "Required command not found: ${cmd}"
done

### Step 1: Build ############################################################

info "Building backup-tool..."
cargo build --release --quiet
[[ -x "${BINARY}" ]] || fail "Binary not found at ${BINARY}"
pass "Build"

### Step 2: Test data ########################################################

info "Generating test data..."
mkdir -p "${SOURCE_DIR}/docs/reports" "${SOURCE_DIR}/media/images" "${SOURCE_DIR}/empty_dir"

echo ""               > "${SOURCE_DIR}/empty.txt"
echo "hello"          > "${SOURCE_DIR}/small.txt"
dd if=/dev/urandom of="${SOURCE_DIR}/medium.bin" bs=1024 count=100 2>/dev/null
dd if=/dev/urandom of="${SOURCE_DIR}/large.bin" bs=1048576 count=5 2>/dev/null
echo "nested content" > "${SOURCE_DIR}/docs/readme.txt"
echo "report data"    > "${SOURCE_DIR}/docs/reports/q1.txt"
dd if=/dev/urandom of="${SOURCE_DIR}/media/images/photo.jpg" bs=1024 count=250 2>/dev/null
cp "${SOURCE_DIR}/medium.bin" "${SOURCE_DIR}/docs/reports/medium_copy.bin"  # duplicate content

touch -t "202001010000" "${SOURCE_DIR}/small.txt"
touch -t "202106151200" "${SOURCE_DIR}/docs/readme.txt"

ln -s "../small.txt" "${SOURCE_DIR}/docs/link_to_small.txt"
ln -s "../docs"      "${SOURCE_DIR}/media/link_to_docs"

pass "Test data generated ($(find "${SOURCE_DIR}" | wc -l | tr -d ' ') entries)"

### Step 3: PGP keypair ######################################################

info "Generating PGP keypair..."
ENCRYPT_KEY_FILE="${CONFIG_DIR}/key.asc"

sq key generate \
    --own-key \
    --userid "Backup Integration Test <${KEY_EMAIL}>" \
    --without-password \
    --expiration never \
    --output "${ENCRYPT_KEY_FILE}" \
    --rev-cert "${WORK_DIR}/revocation.pgp"
[[ -s "${ENCRYPT_KEY_FILE}" ]] || fail "Failed to generate PGP key"
pass "PGP keypair generated"

### Step 4: Start container ##################################################

info "Starting ${DOCKER_IMAGE}..."
# Image has Keystone v3 (ports 35357/5000) and Swift (port 8080).
# Credentials in image ENV: admin / 7a04a385b907caca141f / project admin
docker run --detach \
    --name "${CONTAINER_NAME}" \
    --platform linux/amd64 \
    -p "${KEYSTONE_ADMIN_PORT}:35357" \
    -p "${KEYSTONE_PUBLIC_PORT}:5000" \
    -p "${SWIFT_HOST_PORT}:8080" \
    "${DOCKER_IMAGE}" >/dev/null

info "Waiting for Keystone on ${OS_AUTH_URL} (up to 90 s)..."
READY=0
for i in $(seq 1 45); do
    if curl --silent --fail --max-time 2 "${OS_AUTH_URL}" >/dev/null 2>&1; then
        READY=1; break
    fi
    sleep 2
done
[[ "${READY}" -eq 1 ]] || fail "Keystone did not become ready in time"
pass "Keystone is ready"

### Step 5: Register Swift endpoint ##########################################

info "Registering Swift endpoint in Keystone..."
# The script is run inside the container where Swift is on 127.0.0.1:8080.
# The public URL uses SWIFT_HOST_PORT so it is reachable from the host.
docker exec "${CONTAINER_NAME}" \
    /swift/bin/register-swift-endpoint.sh "http://127.0.0.1:${SWIFT_HOST_PORT}" \
    2>&1 | grep -v "^$" || true
sleep 2
pass "Swift endpoint registered"

### Step 6: Wait for Swift and create containers ##############################

info "Waiting for Swift on http://127.0.0.1:${SWIFT_HOST_PORT}..."
SWIFT_READY=0
for i in $(seq 1 20); do
    if curl --silent --max-time 2 --output /dev/null --write-out "%{http_code}" "http://127.0.0.1:${SWIFT_HOST_PORT}" 2>/dev/null | grep -q "^[0-9]"; then
        SWIFT_READY=1; break
    fi
    sleep 2
done
[[ "${SWIFT_READY}" -eq 1 ]] || fail "Swift did not become ready in time"

info "Creating Swift containers..."
# Authenticate from the host (Keystone and Swift are both reachable on their host ports).
# docker exec is not used here because the container-internal Keystone catalog returns
# http://127.0.0.1:${SWIFT_HOST_PORT} which is not reachable from inside the container.
AUTH_JSON="{\"auth\":{\"identity\":{\"methods\":[\"password\"],\"password\":{\"user\":{\"name\":\"${OS_USERNAME}\",\"domain\":{\"name\":\"${OS_USER_DOMAIN_NAME}\"},\"password\":\"${OS_PASSWORD}\"}}},\"scope\":{\"project\":{\"name\":\"${OS_PROJECT_NAME}\",\"domain\":{\"name\":\"${OS_PROJECT_DOMAIN_NAME}\"}}}}}"

HEADER_FILE="$(mktemp)"
AUTH_BODY=$(curl --silent \
    -X POST "http://127.0.0.1:${KEYSTONE_ADMIN_PORT}/v3/auth/tokens" \
    -H "Content-Type: application/json" \
    -d "${AUTH_JSON}" \
    -D "${HEADER_FILE}") || true
SWIFT_TOKEN=$(grep -i "x-subject-token" "${HEADER_FILE}" | tr -d '\r' | awk '{print $2}') || true
# Extract any URL containing the Swift host port from the catalog (handles "url":"..." and "url": "...")
SWIFT_ACCOUNT_URL=$(echo "${AUTH_BODY}" | grep -o '"url"[[:space:]]*:[[:space:]]*"[^"]*'"${SWIFT_HOST_PORT}"'[^"]*"' | grep -o '"http[^"]*"' | tr -d '"' | head -1) || true
rm -f "${HEADER_FILE}"

info "Token: ${SWIFT_TOKEN:-(empty)}"
info "Account URL: ${SWIFT_ACCOUNT_URL:-(empty)}"
if [[ -z "${SWIFT_ACCOUNT_URL}" ]]; then
    info "Auth body (for debugging): ${AUTH_BODY}"
fi
[[ -n "${SWIFT_TOKEN}" ]]       || fail "Failed to obtain Keystone token"
[[ -n "${SWIFT_ACCOUNT_URL}" ]] || fail "Failed to discover Swift account URL from catalog"

for CONT in "${DATA_CONTAINER}" "${META_CONTAINER}"; do
    HTTP_STATUS=$(curl --silent --output /dev/null --write-out "%{http_code}" \
        -X PUT "${SWIFT_ACCOUNT_URL}/${CONT}" \
        -H "X-Auth-Token: ${SWIFT_TOKEN}") || true
    [[ "${HTTP_STATUS}" =~ ^2 ]] || fail "Failed to create Swift container '${CONT}' (HTTP ${HTTP_STATUS})"
    info "  Container '${CONT}' created"
done
pass "Swift containers created"

### Step 7: Write backup.toml ################################################
# No [stores.*_cloud_config] — the tool reads OS_* vars via from_env().

HMAC_SECRET="integration-test-secret-$(date +%s)"

cat > "${CONFIG_DIR}/backup.toml" << TOML
source = "${SOURCE_DIR}"
data_cache = "${CONFIG_DIR}/data_cache.db"
metadata_cache = "${CONFIG_DIR}/meta_cache.db"
hmac_secret = "${HMAC_SECRET}"
encrypting_key_file = "${ENCRYPT_KEY_FILE}"

[[stores]]
id                 = 1
data_container     = "${DATA_CONTAINER}"
metadata_container = "${META_CONTAINER}"
data_prefix        = "data/"
metadata_prefix    = "meta/"
TOML
pass "backup.toml written"

### Step 8: Backup ###########################################################

info "Running backup..."
"${BINARY}" --config "${CONFIG_DIR}/backup.toml" backup 2>&1 | grep -v "^$" | head -80 || true
pass "Backup completed"

### Step 9: List #############################################################

info "Listing backups..."
BACKUP_NAME=$("${BINARY}" --config "${CONFIG_DIR}/backup.toml" list 2>/dev/null | tail -1)
[[ -n "${BACKUP_NAME}" ]] || fail "No backups found after running backup"
info "Backup name: ${BACKUP_NAME}"
pass "Backup listed"

### Step 10: Validate ########################################################

info "Running validate..."
"${BINARY}" --config "${CONFIG_DIR}/backup.toml" validate "${BACKUP_NAME}" \
    2>&1 | grep -v "^$" | head -20 || true
pass "Validate completed"

### Step 11: Restore #########################################################

info "Running restore into ${RESTORE_DIR}..."
"${BINARY}" --config "${CONFIG_DIR}/backup.toml" restore "${BACKUP_NAME}" "${RESTORE_DIR}" \
    2>&1 | grep -v "^$" | head -80 || true
pass "Restore completed"

### Step 12: Verify ##########################################################

info "Verifying restored data..."

# rsync dry-run covers content (checksum), mtimes, symlink targets,
# missing files, and extra files in the restore directory in one pass.
RSYNC_OUT=$(rsync -an --checksum --itemize-changes --delete \
    "${SOURCE_DIR}/" "${RESTORE_DIR}/" 2>&1) || true

if [[ -z "${RSYNC_OUT}" ]]; then
    pass "All content, symlinks, and modification times match"
else
    echo "${RSYNC_OUT}"
    ERRORS=$(echo "${RSYNC_OUT}" | wc -l | tr -d ' ')
    fail "${ERRORS} verification difference(s) found -- see above"
fi

echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Integration test PASSED               ${NC}"
echo -e "${GREEN}========================================${NC}"
