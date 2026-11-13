#!/bin/sh
# Interim Verus runner (ADR-095, docs/VERIFICATION.md §2.4, §3.3). `cargo xtask proofs verus` wraps the same
# command later; until then this script is the reference invocation.
#
# Usage: proofs/verus/run.sh [--verify-module <mount>] [--output-json <file>] [--min <n>]
#                            [--mutant <file> <find> <replace>] [-- <extra verus args>]
#
#   --verify-module M   verify one mount of proofs/verus/kernels.rs (e.g. snp_proc_xargs_partition)
#   --output-json F     also write Verus's JSON report (func-details, verification-results) to F
#   --min N             fail unless at least N items verify (default 1: a run that verifies nothing fails)
#   --mutant F A B      negative check (V3): verify a temporary copy of the tree in which the first occurrence
#                       of the text A in the kernel file F (repo-relative) is replaced by B; succeeds only if
#                       verification FAILS, and fails if A does not occur in F
#
# Needs `verus` on PATH (the pinned release of proofs/registry.toml; it brings its own rustc 1.99.1 through
# RUSTUP_TOOLCHAIN and never touches the rustup default). On the owner's machine:
#   . ~/.cache/snp-dev/tools/verus/env.sh
# A whole-root run always uses --no-cheating, --crate-type=lib and --edition=2024, and fails on any error or when
# fewer than --min items verify. Verus rejects --verify-module with --no-cheating, so a module run (an
# iteration aid, never a gate) omits that flag.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)

module=""
json=""
min=1
mutant_file=""
mutant_find=""
mutant_replace=""
while [ $# -gt 0 ]; do
    case "$1" in
        --verify-module) module=$2; shift 2 ;;
        --output-json) json=$2; shift 2 ;;
        --min) min=$2; shift 2 ;;
        --mutant) mutant_file=$2; mutant_find=$3; mutant_replace=$4; shift 4 ;;
        --) shift; break ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *) echo "run.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done

command -v verus >/dev/null 2>&1 || {
    echo "run.sh: verus is not on PATH (see proofs/README.md)" >&2
    exit 2
}

root="$repo/proofs/verus/kernels.rs"
work=""
cleanup() { if [ -n "$work" ]; then rm -rf "$work"; fi; }
trap cleanup EXIT INT TERM

if [ -n "$mutant_file" ]; then
    # Mirror the root, the spec tree and every mounted kernel file into a scratch tree, then mutate one file.
    work=$(mktemp -d "${TMPDIR:-/tmp}/snp-verus-mutant.XXXXXX")
    mkdir -p "$work/proofs"
    cp -R "$repo/proofs/verus" "$work/proofs/verus"
    sed -n 's|^#\[path = "\.\./\.\./\(.*\)"\]$|\1|p' "$root" | while read -r f; do
        mkdir -p "$work/$(dirname "$f")"
        cp "$repo/$f" "$work/$f"
    done
    [ -f "$work/$mutant_file" ] || { echo "run.sh: $mutant_file is not mounted in kernels.rs" >&2; exit 2; }
    MUTANT_FIND=$mutant_find MUTANT_REPLACE=$mutant_replace awk '
        BEGIN { find = ENVIRON["MUTANT_FIND"]; repl = ENVIRON["MUTANT_REPLACE"]; done = 0 }
        {
            if (!done) {
                i = index($0, find)
                if (i > 0) { $0 = substr($0, 1, i - 1) repl substr($0, i + length(find)); done = 1 }
            }
            print
        }
        END { if (!done) exit 3 }
    ' "$repo/$mutant_file" > "$work/$mutant_file" || {
        echo "run.sh: mutant text not found in $mutant_file (stale mutant: write a fresh one): $mutant_find" >&2
        exit 1
    }
    root="$work/proofs/verus/kernels.rs"
fi

if [ -n "$module" ]; then
    # Verus refuses `--verify-module` together with `--no-cheating`, so a module run is an iteration aid only:
    # the gate is always the whole root with `--no-cheating` (the guards of §3.2 still reject cheat tokens).
    echo "run.sh: --verify-module runs without --no-cheating (Verus forbids the combination); not a gate" >&2
    set -- "$root" --crate-type=lib --edition=2024 --verify-module "$module" "$@"
else
    set -- "$root" --crate-type=lib --edition=2024 --no-cheating "$@"
fi

out=$(mktemp "${TMPDIR:-/tmp}/snp-verus-out.XXXXXX")
status=0
verus "$@" --output-json > "$out" 2> "$out.err" || status=$?
if [ -n "$json" ]; then cp "$out" "$json"; fi

# The JSON report is one object; pull the counts out without a JSON tool.
verified=$(tr -d ' \n' < "$out" | sed -n 's/.*"verification-results":{[^}]*"verified":\([0-9]*\).*/\1/p')
errors=$(tr -d ' \n' < "$out" | sed -n 's/.*"verification-results":{[^}]*"errors":\([0-9]*\).*/\1/p')
rm -f "$out"

if [ -n "$mutant_file" ]; then
    rm -f "$out.err"
    if [ "$status" -ne 0 ] || [ "${errors:-1}" != 0 ]; then
        echo "mutant killed: verification fails (verified ${verified:-?}, errors ${errors:-?}) for $mutant_file: $mutant_find -> $mutant_replace"
        exit 0
    fi
    echo "run.sh: MUTANT SURVIVED: verification still succeeds for $mutant_file: $mutant_find -> $mutant_replace" >&2
    exit 1
fi

cat "$out.err" >&2
rm -f "$out.err"
if [ "$status" -ne 0 ] || [ "${errors:-1}" != 0 ] || [ -z "${verified:-}" ]; then
    echo "run.sh: verification FAILED (exit $status, verified ${verified:-?}, errors ${errors:-?})" >&2
    exit 1
fi
if [ "$verified" -lt "$min" ]; then
    echo "run.sh: only $verified item(s) verified, expected at least $min" >&2
    exit 1
fi
echo "verification results:: $verified verified, 0 errors${module:+ (module $module)}"
