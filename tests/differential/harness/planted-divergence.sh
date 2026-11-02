#!/bin/sh
# planted-divergence.sh: a stand-in for the snp binary that is the pre-commit 4.7.0 oracle plus ONE deliberate
# difference, used to prove that the shrinker works end to end (WP-35 acceptance: "the shrinker reduces a planted
# divergence to <= 3 hooks/files and writes a working repro.sh"; crates/snp-difftest/tests/oracle.rs).
#
# The difference: after the oracle ran, when the work tree holds a file named `planted.txt` whose content contains
# `PLANT` and the config names a hook `h7`, one extra line is printed on stdout. Everything else is the oracle's
# bytes, so every other part of a scenario is noise the shrinker has to remove.
#
# `@ORACLE@` is replaced with the absolute path of the oracle's `pre-commit` entry point by the test that copies
# this file.
"@ORACLE@" "$@"
rc=$?
if [ -f planted.txt ] && grep -q PLANT planted.txt 2>/dev/null \
    && [ -f .pre-commit-config.yaml ] && grep -q 'id: h7,' .pre-commit-config.yaml 2>/dev/null; then
    echo "planted divergence"
fi
exit "$rc"
