#!/usr/bin/env bash
# Single source of truth for the release payload layout.
# Shared by the Arch tar.gz job, the deb job and the rpm job so the
# file list can never drift between package formats.
#
# Usage: install-payload.sh <payload-dir> <app-binary> <daemon-binary> <portal-binary>
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ $# -ne 4 ]]; then
    echo "usage: $0 <payload-dir> <app-binary> <daemon-binary> <portal-binary>" >&2
    exit 1
fi

PAYLOAD_DIR="$1"
APP_BINARY="$2"
DAEMON_BINARY="$3"
PORTAL_BINARY="$4"

APP_NAME=bennu
DAEMON_BINARY_NAME=bennu-searchd
ACTIVATION_SERVICE_FILE=io.github.nsjsv.Bennu.service
PORTAL_BINARY_NAME=bennu-portal
PORTAL_BUS_SERVICE_FILE=org.freedesktop.impl.portal.desktop.bennu.service
PORTAL_DECLARATION_FILE=bennu.portal

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
install -Dm755 "${PORTAL_BINARY}" "${PAYLOAD_DIR}/usr/bin/${PORTAL_BINARY_NAME}"
install -Dm644 "${REPO_ROOT}/packaging/linux/${PORTAL_BUS_SERVICE_FILE}" \
    "${PAYLOAD_DIR}/usr/share/dbus-1/services/${PORTAL_BUS_SERVICE_FILE}"
# 不写 UseIn：仅当用户在 portals.conf 显式选择 bennu 时才接管 FileChooser。
install -Dm644 "${REPO_ROOT}/packaging/linux/${PORTAL_DECLARATION_FILE}" \
    "${PAYLOAD_DIR}/usr/share/xdg-desktop-portal/portals/${PORTAL_DECLARATION_FILE}"

test -x "${PAYLOAD_DIR}/usr/bin/${APP_NAME}"
test -x "${PAYLOAD_DIR}/usr/lib/${APP_NAME}/${DAEMON_BINARY_NAME}"
test -f "${PAYLOAD_DIR}/usr/lib/systemd/user/bennu-search.service"
test -f "${PAYLOAD_DIR}/usr/share/licenses/${APP_NAME}/LICENSE"
test -f "${PAYLOAD_DIR}/usr/share/applications/${APP_NAME}.desktop"
test -f "${PAYLOAD_DIR}/usr/share/icons/hicolor/512x512/apps/${APP_NAME}.png"
test -f "${PAYLOAD_DIR}/usr/share/${APP_NAME}/matugen/bennu-colors.toml"
test -f "${PAYLOAD_DIR}/usr/share/doc/${APP_NAME}/matugen.md"
test -f "${PAYLOAD_DIR}/usr/share/dbus-1/services/${ACTIVATION_SERVICE_FILE}"
test -x "${PAYLOAD_DIR}/usr/bin/${PORTAL_BINARY_NAME}"
test -f "${PAYLOAD_DIR}/usr/share/dbus-1/services/${PORTAL_BUS_SERVICE_FILE}"
test -f "${PAYLOAD_DIR}/usr/share/xdg-desktop-portal/portals/${PORTAL_DECLARATION_FILE}"
