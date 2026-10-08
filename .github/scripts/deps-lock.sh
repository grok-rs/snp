#!/usr/bin/env bash
# deps.lock helper (ADR-061; owned by WP-00). Used by every workflow and runnable locally.
#
#   deps-lock.sh list     [LOCK]   one line per crate: name<TAB>path<TAB>repository<TAB>branch<TAB>rev
#   deps-lock.sh checkout [LOCK]   check every sibling user crate out at its pinned rev (paths relative to the
#                                  lock's directory, e.g. ../cfgv-rs); an existing checkout at the right rev is kept
#   deps-lock.sh drift    [LOCK]   compare each rev with the head of its branch; prints a Markdown report and exits
#                                  1 when any crate drifted (or its branch is missing), 0 when all match
#   deps-lock.sh link     [LOCK]   in a linked git worktree (e.g. .claude/worktrees/<name>, where ../cfgv-rs resolves
#                                  inside .claude/worktrees/), symlink each missing sibling to the main checkout's
#                                  sibling; existing paths are kept
#
# LOCK defaults to deps.lock next to this repository's root. Only `[[crate]]` tables with `key = "value"` lines are
# understood (the format WP-00 writes).
set -euo pipefail

cmd="${1:-}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
lock="${2:-$here/../../deps.lock}"
[ -f "$lock" ] || { echo "deps-lock: no such file: $lock" >&2; exit 2; }
lock_dir="$(cd "$(dirname "$lock")" && pwd)"

list() {
    awk '
        function flush() {
            if (name != "") {
                if (path == "" || repo == "" || branch == "" || rev == "") {
                    printf "deps-lock: incomplete [[crate]] %s\n", name > "/dev/stderr"; bad = 1
                }
                printf "%s\t%s\t%s\t%s\t%s\n", name, path, repo, branch, rev
            }
            name = path = repo = branch = rev = ""
        }
        /^\[\[crate\]\][[:space:]]*$/ { flush(); next }
        /^[a-z]+[[:space:]]*=[[:space:]]*"[^"]*"[[:space:]]*$/ {
            key = $0; sub(/[[:space:]]*=.*/, "", key)
            val = $0; sub(/^[^"]*"/, "", val); sub(/".*$/, "", val)
            if (key == "name") name = val
            else if (key == "path") path = val
            else if (key == "repository") repo = val
            else if (key == "branch") branch = val
            else if (key == "rev") rev = val
        }
        END { flush(); exit bad }
    ' "$lock"
}

checkout() {
    list | while IFS=$'\t' read -r name path repo branch rev; do
        dest="$lock_dir/$path"
        if [ -d "$dest/.git" ] && [ "$(git -C "$dest" rev-parse HEAD 2>/dev/null)" = "$rev" ]; then
            echo "deps-lock: $name already at $rev ($dest)"
            continue
        fi
        if [ -e "$dest" ] && [ -n "$(ls -A "$dest" 2>/dev/null)" ]; then
            echo "deps-lock: $dest exists and is not $name at $rev; refusing to overwrite it" >&2
            exit 1
        fi
        mkdir -p "$dest"
        git -C "$dest" init -q
        git -C "$dest" remote add origin "$repo"
        git -C "$dest" fetch -q --depth 1 origin "$rev"
        git -C "$dest" checkout -q --detach FETCH_HEAD
        got="$(git -C "$dest" rev-parse HEAD)"
        [ "$got" = "$rev" ] || { echo "deps-lock: $name checked out $got, expected $rev" >&2; exit 1; }
        echo "deps-lock: $name at $rev ($dest, branch $branch of $repo)"
    done
}

drift() {
    local status=0 rows=""
    while IFS=$'\t' read -r name path repo branch rev; do
        head="$(git ls-remote "$repo" "refs/heads/$branch" | cut -f1)"
        if [ -z "$head" ]; then
            rows+="| \`$name\` | \`$rev\` | branch \`$branch\` not found in $repo |"$'\n'
            status=1
        elif [ "$head" != "$rev" ]; then
            rows+="| \`$name\` | \`$rev\` | \`$head\` (\`$branch\`) |"$'\n'
            status=1
        fi
    done < <(list)
    if [ "$status" -eq 0 ]; then
        echo "deps.lock matches the heads of every pinned branch."
    else
        echo "deps.lock differs from the user-crate branch heads (ADR-061)."
        echo
        echo "| crate | deps.lock rev | branch head |"
        echo "|---|---|---|"
        printf '%s' "$rows"
        echo
        echo "Bump deps.lock in a dedicated PR that runs every gate (including crates/snp-config/tests/user_crates.rs and crates/snp-files/tests/user_crates.rs)."
    fi
    return "$status"
}

link() {
    local common main
    common="$(git -C "$lock_dir" rev-parse --path-format=absolute --git-common-dir)"
    main="$(dirname "$common")"
    if [ "$main" = "$lock_dir" ]; then
        echo "deps-lock: $lock_dir is the main checkout; use \`checkout\`" >&2
        exit 2
    fi
    list | while IFS=$'\t' read -r name path _repo _branch _rev; do
        dest="$lock_dir/$path"
        src="$main/$path"
        if [ -e "$dest" ] || [ -L "$dest" ]; then
            echo "deps-lock: $name: $dest exists, kept"
        elif [ -d "$src" ]; then
            ln -s "$(cd "$src" && pwd)" "$dest"
            echo "deps-lock: $name: $dest -> $(cd "$src" && pwd)"
        else
            echo "deps-lock: $name: the main checkout has no $src; run \`checkout\` there first" >&2
            exit 1
        fi
    done
}

case "$cmd" in
    list) list ;;
    checkout) checkout ;;
    drift) drift ;;
    link) link ;;
    *) echo "usage: deps-lock.sh list|checkout|drift|link [LOCK]" >&2; exit 2 ;;
esac
