#!/usr/bin/env bash
# Publishes the crates.io publish set of the workspace at VERSION (ADR-088; release.yml `publish-crates`).
#
#   publish-crates.sh VERSION
#
# The publish set is every workspace package that is not `publish = false` (`cargo xtask release gates` checks that it
# is pinned and self-contained). Each package whose VERSION is already in the crates.io index is skipped, so a re-run
# after a partial failure publishes the rest: `cargo publish` alone fails on the first existing version. The rest go
# to one `cargo publish -p … -p …`, which publishes in dependency order and waits for each crate to reach the index
# before its dependents. When crates.io rate-limits (HTTP 429: a first release creates every crate, and new crates
# have a small burst), the script waits PUBLISH_RETRY_SECONDS (default 600) and starts over, at most
# PUBLISH_MAX_ROUNDS (default 30) times. Any other failure stops it.
#
# Needs CARGO_REGISTRY_TOKEN (cargo reads it), cargo, curl and jq. PUBLISH_INDEX replaces the sparse index base URL;
# PUBLISH_CARGO and PUBLISH_CURL replace the two programs (tests).
set -euo pipefail

version="${1:-}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.+-]+)?$ ]] || {
    echo "usage: publish-crates.sh VERSION (got '${version}')" >&2
    exit 2
}
index="${PUBLISH_INDEX:-https://index.crates.io}"
retry="${PUBLISH_RETRY_SECONDS:-600}"
rounds="${PUBLISH_MAX_ROUNDS:-30}"
user_agent="snp-release (https://github.com/grok-rs/snp)"
cargo="${PUBLISH_CARGO:-cargo}"
curl="${PUBLISH_CURL:-curl}"

# The sparse-index path of a (lowercase) crate name: 1/a, 2/ab, 3/a/abc, ab/cd/abcd…
index_path() {
    local n="$1"
    case "${#n}" in
        1) echo "1/$n" ;;
        2) echo "2/$n" ;;
        3) echo "3/${n:0:1}/$n" ;;
        *) echo "${n:0:2}/${n:2:2}/$n" ;;
    esac
}

# Whether NAME VERSION is in the index (a crate the index does not know is not published).
published() {
    local body
    body="$("$curl" --proto '=https' --tlsv1.2 -fsS -A "$user_agent" "$index/$(index_path "$1")" 2>/dev/null)" || return 1
    jq -e --arg v "$version" 'select(.vers == $v)' <<< "$body" > /dev/null
}

meta="$("$cargo" metadata --format-version 1 --no-deps --locked)"
# A read loop rather than `mapfile`: macOS still ships bash 3.2, and the script's tests run there too.
crates=()
while IFS= read -r c; do crates+=("$c"); done < <(jq -r '.packages[] | select(.publish == null) | .name' <<< "$meta" | sort)
[ "${#crates[@]}" -gt 0 ] || { echo "publish-crates: the workspace publishes no crate" >&2; exit 1; }
stray="$(jq -r --arg v "$version" '.packages[] | select(.publish == null and .version != $v) | "\(.name) \(.version)"' <<< "$meta")"
[ -z "$stray" ] || { echo "publish-crates: not at $version: $stray" >&2; exit 1; }
echo "publish set (${#crates[@]}): ${crates[*]}"

log="$(mktemp)"
trap 'rm -f "$log"' EXIT
round=1
while :; do
    pending=()
    for c in "${crates[@]}"; do
        if published "$c"; then
            echo "$c $version is already on crates.io"
        else
            pending+=("$c")
        fi
    done
    if [ "${#pending[@]}" -eq 0 ]; then
        echo "every crate of the publish set is on crates.io at $version"
        exit 0
    fi
    [ -n "${CARGO_REGISTRY_TOKEN:-}" ] || { echo "publish-crates: the CARGO_REGISTRY_TOKEN secret is not set" >&2; exit 1; }
    echo "round $round: publishing ${pending[*]}"
    args=()
    for c in "${pending[@]}"; do args+=(-p "$c"); done
    set +e
    "$cargo" publish --locked "${args[@]}" 2>&1 | tee "$log"
    status="${PIPESTATUS[0]}"
    set -e
    if [ "$status" -eq 0 ]; then
        # cargo waited for each crate to reach the index
        echo "published ${pending[*]}"
        exit 0
    fi
    if ! grep -Eqi '429|too many requests|too many new crates|rate limit' "$log"; then
        echo "publish-crates: cargo publish failed (exit $status)" >&2
        exit "$status"
    fi
    if [ "$round" -ge "$rounds" ]; then
        echo "publish-crates: still rate-limited after $rounds rounds; re-run the job later" >&2
        exit 1
    fi
    echo "::notice::crates.io rate limit; retrying in ${retry}s (round $round of $rounds)"
    sleep "$retry"
    round=$((round + 1))
done
