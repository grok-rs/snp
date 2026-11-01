#!/usr/bin/env bash
# Runs the git-dependent tests against the git on PATH (the git-version axis of .github/workflows/platform.yml; WBS
# WP-37). Run from the repository root, after `cargo build --workspace --bins --features snp/testing`.
#
#   packaging/ci/git-tests.sh VERSION
#
# Every test runs blocking except the ones listed in packaging/ci/git-known-failures/VERSION.txt (one nextest filter
# per line, `#` comments): tests that still assume a newer git until their owners gate them by git version. Those run
# afterwards one by one, without failing the job; one that passes is reported (::warning::) so that its line is
# removed. A regression anywhere else fails the job.
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 VERSION" >&2
  exit 2
fi
version=$1
known="packaging/ci/git-known-failures/$version.txt"

# The crates whose behaviour depends on the git version: the git layer, the test fixtures, the stash guard, the
# store's clones, hook resolution, the run pipeline, the commands and the binary's integration tests.
packages=(-p snp-git -p snp-testkit -p snp-stash -p snp-store -p snp-repo -p snp-run -p snp-cmd -p snp)
common=(--features snp/testing --locked --no-tests=warn)

filters=()
if [ -f "$known" ]; then
  while IFS= read -r line; do
    case "$line" in '' | '#'*) continue ;; esac
    filters+=("$line")
  done < "$known"
fi

if [ "${#filters[@]}" -eq 0 ]; then
  exec cargo nextest run "${common[@]}" --profile ci "${packages[@]}"
fi

expr=""
for f in "${filters[@]}"; do
  expr="${expr:+$expr | }($f)"
done
echo "git $version: ${#filters[@]} known failure(s) run separately (see $known)"
cargo nextest run "${common[@]}" --profile ci "${packages[@]}" -E "not ($expr)"

for f in "${filters[@]}"; do
  if cargo nextest run "${common[@]}" --profile default "${packages[@]}" -E "$f" > /dev/null 2>&1; then
    echo "::warning::git $version: '$f' passes now; remove it from $known"
  else
    echo "known failure on git $version: $f"
  fi
done
