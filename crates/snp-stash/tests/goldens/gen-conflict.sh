#!/bin/sh
# Regenerates the stash-conflict transcript goldens from the pre-commit 4.6.2 oracle (CPython 3.12.3):
#
#   ORACLE=target/oracle/pc462-py312 crates/snp-stash/tests/goldens/gen-conflict.sh
#
# Scenario (02 §7.2 "Observed conflict transcript"): `foo` staged with FOO_CONTENTS, an unstaged `1`→`a`, a local
# `system` hook rewriting `1`→`b`. Writes conflict.full.txt / conflict.color.full.txt (the whole stdout) and
# conflict.txt / conflict.color.txt (the stash lines only), with the patch path replaced by `<PATCH>`.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
oracle=${ORACLE:-/home/dromix/projects/snp/target/oracle/pc462-py312}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export HOME="$work/home" GIT_CONFIG_NOSYSTEM=1 PRE_COMMIT_HOME="$work/store" LC_ALL=C TZ=UTC
mkdir -p "$HOME"
git config --global user.name test
git config --global user.email test@example.com
git config --global init.defaultBranch master
git init -q "$work/r"
cd "$work/r"
printf '1\n2\n3\n4\n5\n6\n7\n8\n' > foo
cat > .pre-commit-config.yaml <<'EOF'
repos:
-   repo: local
    hooks:
    -   id: fixer
        name: fixer
        entry: sed -i s/1/b/
        language: system
        files: ^foo$
EOF
git add foo .pre-commit-config.yaml
printf 'a\n2\n3\n4\n5\n6\n7\n8\n' > foo
for mode in never always; do
    suffix=
    [ "$mode" = always ] && suffix=.color
    "$oracle/bin/pre-commit" run --color "$mode" > "$work/out" 2>&1 || true
    patch=$(sed -n 's/.*Stashing unstaged files to \(.*\)\.$/\1/p' "$work/out")
    sed "s#$patch#<PATCH>#g" "$work/out" > "$here/conflict$suffix.full.txt"
    grep -E 'Unstaged files detected|Stashing unstaged files|Stashed changes conflicted|Restored changes from' \
        "$here/conflict$suffix.full.txt" > "$here/conflict$suffix.txt"
    git checkout -q -- foo 2>/dev/null || true
    printf 'a\n2\n3\n4\n5\n6\n7\n8\n' > foo
done
