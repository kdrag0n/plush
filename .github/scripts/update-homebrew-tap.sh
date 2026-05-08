#!/usr/bin/env bash
set -euo pipefail

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

require_asset() {
  local asset=$1
  [[ -f "${DIST_DIR}/${asset}" ]] || die "missing release asset ${DIST_DIR}/${asset}"
}

DIST_DIR=${DIST_DIR:-dist}
TAP_REPO=${TAP_REPO:-kdrag0n/homebrew-tap}
TAP_BRANCH=${TAP_BRANCH:-main}
FORMULA_PATH=${FORMULA_PATH:-Formula/plush.rb}
PLUSH_REPOSITORY=${PLUSH_REPOSITORY:-${GITHUB_REPOSITORY:-kdrag0n/plush}}
PLUSH_VERSION=${PLUSH_VERSION:-0.1.0}
HOMEBREW_REVISION=${HOMEBREW_REVISION:-${GITHUB_RUN_NUMBER:-}}

[[ -n "${HOMEBREW_REVISION}" ]] || die "HOMEBREW_REVISION or GITHUB_RUN_NUMBER must be set"
[[ "${HOMEBREW_REVISION}" =~ ^[0-9]+$ ]] || die "HOMEBREW_REVISION must be an integer"
[[ -n "${GH_TOKEN:-}" ]] || die "GH_TOKEN must be set"
command -v gh >/dev/null 2>&1 || die "gh CLI is required"

if [[ -n "${RELEASE_TAG:-}" ]]; then
  release_tag=${RELEASE_TAG}
else
  ref_name=${GITHUB_REF_NAME:-main}
  safe_ref=$(printf '%s' "${ref_name}" | tr -c 'A-Za-z0-9._-' '-')
  release_tag="prerelease-${safe_ref}"
fi

assets=(
  plush-macos-aarch64
  plush-macos-x86_64
  plush-linux-aarch64-musl
  plush-linux-x86_64-musl
)

for asset in "${assets[@]}"; do
  require_asset "${asset}"
done

url_base="https://github.com/${PLUSH_REPOSITORY}/releases/download/${release_tag}"
sha_plush_macos_arm=$(sha256_file "${DIST_DIR}/plush-macos-aarch64")
sha_plush_macos_x64=$(sha256_file "${DIST_DIR}/plush-macos-x86_64")
sha_plush_linux_arm=$(sha256_file "${DIST_DIR}/plush-linux-aarch64-musl")
sha_plush_linux_x64=$(sha256_file "${DIST_DIR}/plush-linux-x86_64-musl")

workdir=$(mktemp -d)
trap 'rm -rf "${workdir}"' EXIT

export GH_CONFIG_DIR=${GH_CONFIG_DIR:-"${workdir}/gh"}
mkdir -p "${GH_CONFIG_DIR}"
gh config set git_protocol https --host github.com
gh auth setup-git --hostname github.com
gh repo clone "${TAP_REPO}" "${workdir}/tap" -- --branch "${TAP_BRANCH}"

formula="${workdir}/tap/${FORMULA_PATH}"
mkdir -p "$(dirname "${formula}")"

cat > "${formula}" <<EOF
class Plush < Formula
  desc "Soft comfy bash-compatible shell"
  homepage "https://github.com/${PLUSH_REPOSITORY}"
  version "${PLUSH_VERSION}"
  revision ${HOMEBREW_REVISION}
  license "MIT"

  if OS.mac? && Hardware::CPU.arm?
    url "${url_base}/plush-macos-aarch64",
        using: :nounzip
    sha256 "${sha_plush_macos_arm}"
  elsif OS.mac? && Hardware::CPU.intel?
    url "${url_base}/plush-macos-x86_64",
        using: :nounzip
    sha256 "${sha_plush_macos_x64}"
  elsif OS.linux? && Hardware::CPU.arm?
    url "${url_base}/plush-linux-aarch64-musl",
        using: :nounzip
    sha256 "${sha_plush_linux_arm}"
  elsif OS.linux? && Hardware::CPU.intel?
    url "${url_base}/plush-linux-x86_64-musl",
        using: :nounzip
    sha256 "${sha_plush_linux_x64}"
  else
    odie "plush prebuilt binaries are not available for this platform"
  end

  def install
    bin.install cached_download => "plush"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/plush --version")
    assert_equal "hello\n", shell_output("#{bin}/plush -c 'echo hello'")
  end
end
EOF

ruby -c "${formula}"

git -C "${workdir}/tap" config user.name "github-actions[bot]"
git -C "${workdir}/tap" config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git -C "${workdir}/tap" add "${FORMULA_PATH}"

if git -C "${workdir}/tap" diff --cached --quiet; then
  printf 'Homebrew formula is already up to date.\n'
  exit 0
fi

git -C "${workdir}/tap" commit -m "Update plush prerelease formula"
git -C "${workdir}/tap" push origin "HEAD:${TAP_BRANCH}"
