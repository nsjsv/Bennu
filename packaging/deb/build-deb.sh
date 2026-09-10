#!/usr/bin/env bash
# Build the Debian/Ubuntu .deb inside an ubuntu:24.04 container.
# The produced binary links against noble's glibc and shared libraries,
# so the package installs on Ubuntu 24.04+ and Debian 13+.
# (noble is the oldest base shipping dav1d >= 1.3.0, required by
# the AVIF decoder.)
#
# Usage: build-deb.sh <semver>   (run from anywhere; repo root is derived)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ $# -ne 1 ]] || ! [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: $0 <x.y.z>" >&2
    exit 1
fi
SEMVER="$1"

APP_NAME=bennu
DEB_ARCH=amd64
DEB_REVISION=1

cd "${REPO_ROOT}"

require_matching_cargo_version() {
    local package_name="$1"
    local package_version
    package_version="$(cargo pkgid -p "${package_name}")"
    package_version="${package_version##*#}"
    if [[ "${package_version}" == "${package_version#*@}" ]]; then
        package_version="${package_version##*@}"
    fi
    if [[ "${package_version}" != "${SEMVER}" ]]; then
        echo "::error::${package_name} version ${package_version} does not match release ${SEMVER}" >&2
        exit 1
    fi
}

install_build_dependencies() {
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        dpkg-dev \
        file \
        libacl1-dev \
        libasound2-dev \
        libdav1d-dev \
        libfontconfig1-dev \
        libglib2.0-dev \
        libnotify-dev \
        libwayland-dev \
        libxkbcommon-dev \
        pkg-config
}

install_rust_toolchain() {
    if ! command -v cargo >/dev/null 2>&1; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --profile minimal --default-toolchain stable
        export PATH="${HOME}/.cargo/bin:${PATH}"
    fi
}

DEB_DEPENDS=(
    libacl1
    libasound2t64
    libdav1d7
    libfontconfig1
    libglib2.0-0t64
    libnotify4
    libwayland-client0
    libxkbcommon-x11-0
    libxkbcommon0
    wl-clipboard
    xdg-utils
)

DEB_SUGGESTS=(
    ffmpeg
    ffmpegthumbnailer
    gvfs-backends
    libreoffice
    libsecret
    p7zip-full
    poppler-utils
)

build_control_file() {
    local control_dir="$1"
    local depends suggests

    depends="$(IFS=, ; echo "${DEB_DEPENDS[*]}")"
    suggests="$(IFS=, ; echo "${DEB_SUGGESTS[*]}")"

    cat > "${control_dir}/control" <<CONTROL
Package: ${APP_NAME}
Version: ${SEMVER}-${DEB_REVISION}
Section: utils
Priority: optional
Architecture: ${DEB_ARCH}
Maintainer: nsjsv <bugmojang@gmail.com>
Depends: ${depends}
Suggests: ${suggests}
Description: Linux desktop file manager written in Rust and Iced
 Multi-column file manager with previews (text, Markdown, archives, PDF,
 Office, images, audio and video), virtualized large-directory views and an
 optional indexed search service.
CONTROL
}

install_build_dependencies
install_rust_toolchain
require_matching_cargo_version app-ui
require_matching_cargo_version file-search

target_dir="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}"
CARGO_TARGET_DIR="${target_dir}" cargo build --release --locked -p app-ui -p file-search

dist_dir="${REPO_ROOT}/dist"
payload_root="$(mktemp -d)"
deb_dir="${payload_root}/${APP_NAME}_${SEMVER}-${DEB_REVISION}_${DEB_ARCH}"
mkdir -p "${deb_dir}/DEBIAN"

bash "${REPO_ROOT}/packaging/common/install-payload.sh" \
    "${deb_dir}" \
    "${target_dir}/release/app-ui" \
    "${target_dir}/release/bennu-searchd"
build_control_file "${deb_dir}/DEBIAN"

mkdir -p "${dist_dir}"
dpkg-deb --build --root-owner-group "${deb_dir}" \
    "${dist_dir}/${APP_NAME}_${SEMVER}-${DEB_REVISION}_${DEB_ARCH}.deb"

(
    cd "${dist_dir}"
    sha256sum "${APP_NAME}_${SEMVER}-${DEB_REVISION}_${DEB_ARCH}.deb" > SHA256SUMS.deb
)

echo "built: ${dist_dir}/${APP_NAME}_${SEMVER}-${DEB_REVISION}_${DEB_ARCH}.deb"
