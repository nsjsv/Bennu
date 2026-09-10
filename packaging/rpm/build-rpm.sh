#!/usr/bin/env bash
# Build the Fedora .rpm inside a Fedora container.
# Library dependencies are generated automatically by rpmbuild from the
# binary sonames; the spec only pins runtime commands and weak suggests.
#
# Usage: build-rpm.sh <semver>   (run from anywhere; repo root is derived)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ $# -ne 1 ]] || ! [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: $0 <x.y.z>" >&2
    exit 1
fi
SEMVER="$1"

APP_NAME=bennu

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
    dnf install -y --setopt=install_weak_deps=False \
        gcc \
        curl \
        ca-certificates \
        file \
        pkgconf-pkg-config \
        rpm-build \
        libacl-devel \
        alsa-lib-devel \
        libdav1d-devel \
        fontconfig-devel \
        glib2-devel \
        libnotify-devel \
        libxkbcommon-devel \
        wayland-devel
}

install_rust_toolchain() {
    if ! command -v cargo >/dev/null 2>&1; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --profile minimal --default-toolchain stable
        export PATH="${HOME}/.cargo/bin:${PATH}"
    fi
}

install_build_dependencies
install_rust_toolchain
require_matching_cargo_version app-ui
require_matching_cargo_version file-search

target_dir="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}"
CARGO_TARGET_DIR="${target_dir}" cargo build --release --locked -p app-ui -p file-search

dist_dir="${REPO_ROOT}/dist"
mkdir -p "${dist_dir}"

spec_path="${dist_dir}/bennu.spec"
sed \
    -e "s|@SEMVER@|${SEMVER}|g" \
    -e "s|@PAYLOAD_SCRIPT@|${REPO_ROOT}/packaging/common/install-payload.sh|g" \
    -e "s|@APP_BINARY@|${target_dir}/release/app-ui|g" \
    -e "s|@DAEMON_BINARY@|${target_dir}/release/bennu-searchd|g" \
    "${REPO_ROOT}/packaging/rpm/bennu.spec.in" > "${spec_path}"

rpmbuild --quiet -bb \
    --define "_topdir $(mktemp -d)" \
    --define "_rpmdir ${dist_dir}" \
    --define "_buildrootdir ${dist_dir}/rpm-buildroot" \
    "${spec_path}"

rm -rf "${dist_dir}/rpm-buildroot"
rpm_path="${dist_dir}/x86_64/${APP_NAME}-${SEMVER}-1.*.rpm"
test -n "$(compgen -G "${rpm_path}")"

(
    cd "${dist_dir}"
    sha256sum x86_64/*.rpm > SHA256SUMS.rpm
)

echo "built: ${dist_dir}/x86_64/"
