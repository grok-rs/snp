#!/usr/bin/env bash
# Prints snp's cache directory the way snp resolves it (crates/snp-store/src/location.rs): SNP_HOME, else
# $XDG_CACHE_HOME/snp, else ~/.cache/snp; an empty variable counts as unset. `~` is left for actions/cache to expand
# (on Windows to %USERPROFILE%, as snp does). A relative value is relative to the step's working directory, which is
# also where actions/cache resolves it.
set -euo pipefail

if [ -n "${SNP_HOME:-}" ]; then
  printf '%s\n' "$SNP_HOME"
elif [ -n "${XDG_CACHE_HOME:-}" ]; then
  printf '%s\n' "${XDG_CACHE_HOME%/}/snp"
else
  printf '%s\n' '~/.cache/snp'
fi
