# D-SEC-05

installers and owned repos get pre-commit's no_git_env allow-list

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-18 (contributing: WP-24, WP-25); the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-05.yaml` (WBS "Divergence files", ADR-071).
* **Status:** owned repositories implemented by WP-18 (`snp_git::no_git_env`, `snp_git::OwnedRepo`,
  `crates/snp-git/src/owned.rs`; 02 G.7 rows 4–5, ADR-018); the installer half is WP-24/WP-25's
  (`installer_env = no_git_env(hook_env)`, ARCHITECTURE §2.5).

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| installers and snp-owned repo operations inherit `GIT_DIR`/`GIT_INDEX_FILE` (index corruption in linked worktrees; autoupdate writes partial-clone config into the user's repo — hazard c) | installers and owned repos get pre-commit's `no_git_env` allow-list |

## Rationale

git exports `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE` while it runs hooks, and shells export `GIT_DIR` for
dotfile setups and `git --git-dir` wrappers. pre-commit strips them for its store clone (`git.no_git_env`,
`git.py:27-48`) but not for `autoupdate`'s temporary repository after `init_repo` (`autoupdate.py:40-72`: `config`,
`fetch`, `describe`, `tag`, `rev-parse`, `show`, `checkout` run with the full environment), nor for language
installers (`pip`, `npm`, `go install` building packages with git). With `GIT_DIR` exported, `git -C <tmp> config
extensions.partialClone true` writes into the **user's** repository, the following `fetch` from `origin` fails, and
the user is left with `repositoryformatversion = 1`, `extensions.partialClone = true` and a promisor remote that
changes how every later fetch of their repository behaves (02 G.3 hazard c). In a linked worktree an installer that
runs git with the hook's `GIT_INDEX_FILE` writes the wrong index (prek #2505).

snp runs **every** git call on a repository it owns — store clones, deps variants, autoupdate, the try-repo shadow —
with `no_git_env(base)`: pre-commit's own allow-list (every `GIT_*` variable dropped except `GIT_EXEC_PATH`,
`GIT_SSH`, `GIT_SSH_COMMAND`, `GIT_SSL_CAINFO`, `GIT_SSL_NO_VERIFY`, `GIT_CONFIG_COUNT`/`KEY_*`/`VALUE_*`,
`GIT_HTTP_PROXY_AUTHMETHOD`, `GIT_ALLOW_PROTOCOL`, `GIT_ASKPASS`; names upper-cased on Windows like CPython's
`os.environ` copy, ADR-074), plus `core.fsmonitor=` and `core.useBuiltinFSMonitor=false` appended through
`GIT_CONFIG_COUNT` (an existing count is extended, a malformed one left alone). The argv stays pre-commit's (error
texts are unchanged); only the environment of calls on snp's own repositories differs, and only where pre-commit
reads or writes the wrong repository. User-repository calls keep the full environment (parity, 02 G.6).

## Examples

A hooks repository with tags `v1.0.0`, `v2.0.0`; the user repository's config pins `rev: v1.0.0`; the shell exports
`GIT_DIR=<user>/.git`; `pre-commit autoupdate`.

pre-commit 4.6.2 (oracle `pc462-py312`, git 2.43.0), exit **3**, stdout:

```
An unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', '-c', 'core.useBuiltinFSMonitor=false', '-C', '/tmp/tmpsblpeqct', 'fetch', 'origin', 'HEAD', '--quiet', '--filter=blob:none', '--tags')
return code: 128
stdout: (none)
stderr:
    fatal: 'origin' does not appear to be a git repository
    fatal: Could not read from remote repository.
    
    Please make sure you have the correct access rights
    and the repository exists.
Check the log at <store>/pre-commit.log
```

and the user's `.git/config` afterwards:

```diff
-	repositoryformatversion = 0
+	repositoryformatversion = 1
+[extensions]
+	partialClone = true
+[remote "origin"]
+	promisor = true
+	partialclonefilter = blob:none
```

snp: `[<hooks>] updating v1.0.0 -> v2.0.0`, the config is rewritten, exit **0**; the user's `.git/config` is
byte-identical before and after.

Regression tests: `crates/snp-git/tests/behaviour/hazards.rs`
`hazard_c_git_dir_never_redirects_autoupdate_into_the_user_repo` (real git, `GIT_DIR`/`GIT_WORK_TREE`/
`GIT_INDEX_FILE` exported; the user repository is unchanged), `crates/snp-git/tests/behaviour/owned.rs`
`owned_env_is_sanitized_and_carries_the_fsmonitor_injection` (the environment of every owned call) and the catalog
row T10-GIT-12 (`no_git_env`).
