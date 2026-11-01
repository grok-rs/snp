#!/usr/bin/env bash
# The "Run snp" step of snp-action (action.yml).
#
# pre-commit/action runs `pre-commit run --show-diff-on-failure --color=always ${{ inputs.extra_args }}`: the input is
# pasted into the bash script, so bash parses it - quotes group words (`--files "a b.py"`), and `$VAR` and `$(...)`
# expand (`--from-ref "$BASE"`, `--files $(git diff --name-only HEAD~1)`). snp-action receives the input through the
# environment (SNP_ACTION_EXTRA_ARGS) and has bash parse it the same way with `eval "set -- ..."`, so every
# extra_args that works with pre-commit/action works here with the same meaning.
#
# Trust model: the same as pre-commit/action's. extra_args is shell syntax written by the workflow's author; a
# workflow must never put untrusted text (a pull-request title, a branch name) into it, with either action.
set -euo pipefail

eval "set -- ${SNP_ACTION_EXTRA_ARGS:-}"
exec "${SNP_ACTION_SNP:-snp}" run --show-diff-on-failure --color=always "$@"
