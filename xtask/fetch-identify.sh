#!/usr/bin/env bash
# Download a Python identify wheel from PyPI, verify its sha256 and unpack it
# (a wheel is a zip archive: nothing is installed or executed).
#
#   xtask/fetch-identify.sh <version|latest> <dest-dir>
#
# Prints the path of the unpacked `identify` package directory, suitable for
# `cargo xtask gen-identify --src <path>`.
set -euo pipefail

version="${1:?usage: fetch-identify.sh <version|latest> <dest-dir>}"
dest="${2:?usage: fetch-identify.sh <version|latest> <dest-dir>}"

if [ "$version" = latest ]; then
  api="https://pypi.org/pypi/identify/json"
else
  api="https://pypi.org/pypi/identify/${version}/json"
fi

meta="$(curl -fsSL "$api")"
wheel_url="$(jq -r '[.urls[] | select(.packagetype == "bdist_wheel")][0].url' <<<"$meta")"
wheel_sha="$(jq -r '[.urls[] | select(.packagetype == "bdist_wheel")][0].digests.sha256' <<<"$meta")"
if [ -z "$wheel_url" ] || [ "$wheel_url" = null ]; then
  echo "no wheel found for identify ${version}" >&2
  exit 1
fi

mkdir -p "$dest"
wheel="$dest/$(basename "$wheel_url")"
curl -fsSL "$wheel_url" -o "$wheel"
echo "${wheel_sha}  ${wheel}" | sha256sum --check --quiet
unzip -q -o "$wheel" -d "$dest/unpacked"
echo "$dest/unpacked/identify"
