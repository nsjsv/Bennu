#!/usr/bin/env bash
# Single source of truth for the release payload layout.
# Shared by the Arch tar.gz job, the deb job and the rpm job so the
# file list can never drift between package formats.
#
# Usage: install-payload.sh <payload-dir> <app-binary> <daemon-binary>
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ $# -ne 3 ]]; then
    echo "usage: $0 <payload-dir> <app-binary> <daemon-binary>" >&2
    exit 1
fi

PAYLOAD_DIR="$1"
APP_BINARY="$2"
DAEMON_BINARY="$3"

APP_NAME=bennu
DAEMON_BINARY_NAME=bennu-searchd
ACTIVATION_SERVICE_FILE=io.github.nsjsv.Bennu.service

install -Dm755 "${APP_BINARY}" "${PAYLOAD_DIR}/usr/bin/${APP_NAME}"
install -Dm755 "${DAEMON_BINARY}" "${PAYLOAD_DIR}/usr/lib/${APP_NAME}/${DAEMON_BINARY_NAME}"
install -Dm644 "${REPO_ROOT}/packaging/linux/bennu-search.service" \
    "${PAYLOAD_DIR}/usr/lib/systemd/user/bennu-search.service"
install -Dm644 "${REPO_ROOT}/LICENSE" \
    "${PAYLOAD_DIR}/usr/share/licenses/${APP_NAME}/LICENSE"
install -Dm644 "${REPO_ROOT}/packaging/linux/bennu.desktop" \
    "${PAYLOAD_DIR}/usr/share/applications/${APP_NAME}.desktop"
install -Dm644 "${REPO_ROOT}/packaging/linux/icons/hicolor/512x512/apps/bennu.png" \
    "${PAYLOAD_DIR}/usr/share/icons/hicolor/512x512/apps/${APP_NAME}.png"
install -Dm644 "${REPO_ROOT}/packaging/matugen/bennu-colors.toml" \
    "${PAYLOAD_DIR}/usr/share/${APP_NAME}/matugen/bennu-colors.toml"
install -Dm644 "${REPO_ROOT}/packaging/matugen/README.md" \
    "${PAYLOAD_DIR}/usr/share/doc/${APP_NAME}/matugen.md"
install -Dm644 "${REPO_ROOT}/packaging/linux/${ACTIVATION_SERVICE_FILE}" \
    "${PAYLOAD_DIR}/usr/share/dbus-1/services/${ACTIVATION_SERVICE_FILE}"

test -x "${PAYLOAD_DIR}/usr/bin/${APP_NAME}"
test -x "${PAYLOAD_DIR}/usr/lib/${APP_NAME}/${DAEMON_BINARY_NAME}"
test -f "${PAYLOAD_DIR}/usr/lib/systemd/user/bennu-search.service"
test -f "${PAYLOAD_DIR}/usr/share/licenses/${APP_NAME}/LICENSE"
test -f "${PAYLOAD_DIR}/usr/share/applications/${APP_NAME}.desktop"
test -f "${PAYLOAD_DIR}/usr/share/icons/hicolor/512x512/apps/${APP_NAME}.png"
test -f "${PAYLOAD_DIR}/usr/share/${APP_NAME}/matugen/bennu-colors.toml"
test -f "${PAYLOAD_DIR}/usr/share/doc/${APP_NAME}/matugen.md"
test -f "${PAYLOAD_DIR}/usr/share/dbus-1/services/${ACTIVATION_SERVICE_FILE}"
