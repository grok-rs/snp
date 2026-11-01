# Migrating from pre-commit

Your `.pre-commit-config.yaml` needs no change: snp reads it exactly as pre-commit 4.6.2 does. What changes is *which
program git runs* when it fires a hook, which program your scripts and CI call, and which cache directory is used.
Every step below can be undone (see [Rollback](#rollback)), and until you run `snp install` (or create the alias)
installing snp changes nothing at all.

## Checklist

1. **Install snp** ([Installation](installation.md)) and check `snp --version` prints `snp 2.0.0`.
2. **Inspect a repository** with `snp doctor`. For each of the ten hook types it shows which tool git runs today
   (Python pre-commit, prek, snp 1.x, snp, a foreign script or nothing) and predicts whether commits keep working.
3. **Switch the repository's hook scripts:**
   * `snp install --install-hooks` installs the default hook types (those of `default_install_hook_types`, else
     `pre-commit`) and builds every hook environment now, so the first commit is fast; or
   * `snp doctor --fix` switches **every** hook type pre-commit or prek had installed, keeping their options
     (`--config`, `--allow-missing-config`).
4. **Template directory and config hooks.** If you used `pre-commit init-templatedir DIR`, run
   `snp init-templatedir DIR`. If you use git ≥ 2.54 config-based hooks, point them at snp (see
   [Template directories and config hooks](#template-directories-and-config-hooks)).
5. **Scripts and tools that call `pre-commit`** (Makefiles, tox, nox, IDE settings, docs): replace `pre-commit` with
   `snp`, or keep the name with `snp self alias install`
   ([details](#scripts-and-tools-that-call-pre-commit)).
6. **CI:** replace `pre-commit/action` with `snp-action` (same `extra_args`) or the snp steps, and **change every cache
   key** — an immutable cache restored from pre-commit's key would never save snp's cache. See
   [CI integration](ci.md).
7. **pre-commit.ci** keeps working: it runs Python pre-commit on its own infrastructure. Keep the config valid for
   Python pre-commit while anyone still uses it (see [Configuration](configuration.md#sharing-a-config-with-pre-commit)).
8. **Optional:** remove Python pre-commit (`pipx uninstall pre-commit`, …) and its cache (`pre-commit clean` before
   uninstalling, or delete `~/.cache/pre-commit`). Do step 3 in every repository first: a hook script still written
   by pre-commit then fails with `` `pre-commit` not found. `` (or `No module named pre_commit` for `pip --user`,
   system and project-venv installs) unless the alias exists.
9. **Rollback** is possible at any time ([Rollback](#rollback)).

## What `snp install` does to existing hook scripts

pre-commit refuses to overwrite a hook script it does not recognise and moves it to `<hook>.legacy` instead. snp does
the same for foreign scripts (your own hooks), but it **recognises the scripts of pre-commit, prek and snp 1.x** and
replaces them in place, keeping their options, without creating a `.legacy` chain that would run the old tool too:

```console
$ snp install
Replaced hook script from pre-commit at .git/hooks/pre-commit (Python: /home/me/.local/pipx/venvs/pre-commit/bin/python)
snp installed at .git/hooks/pre-commit
```

* A `.legacy` file that is itself a manager script (left over by a previous migration) is removed and reported:
  `Removed .git/hooks/pre-commit.legacy: it is a hook script from pre-commit, not a user hook`.
* Other hook types that still run another tool are listed:
  ``Note: .git/hooks/pre-push still runs pre-commit; run `snp install -t pre-push` to switch it to snp``.
* A user hook in `.legacy` keeps running first, exactly as with pre-commit ("migration mode").

The script snp writes is a small POSIX `sh` script (it also works under the BusyBox `ash` some Git GUIs bundle) that
embeds the absolute path of the `snp` it was installed with, on a stable path such as `~/.cargo/bin/snp` or
`/opt/homebrew/bin/snp` rather than a versioned install directory, so upgrades keep commits working and GUI clients
with a minimal `PATH` still find snp. If that path disappears the script falls back to `snp` on `PATH`, then fails with
`` `snp` not found.  Did you forget to activate your virtualenv? ``.

`core.hooksPath` set (for example by husky) makes `install` refuse exactly as pre-commit does; `snp doctor` names the
setting and its origin.

## Template directories and config hooks

`pre-commit init-templatedir DIR` writes hook scripts into `DIR/hooks/` that **every future clone** copies. Run

```sh
snp init-templatedir ~/.git-template
```

to overwrite them with snp's. Clones made earlier keep their copies: fix them per repository with `snp install` or
`snp doctor --fix`.

git ≥ 2.54 supports hooks declared in the git configuration. pre-commit's documentation uses the friendly names
`hook.pre-commit.<type>`; re-point them under the **same name**, so the entry is replaced instead of duplicated:

```sh
git config set --global hook.pre-commit.pre-commit.command 'snp hook-impl --hook-type pre-commit --'
```

`snp install --git-config-hooks` writes these entries for the current repository. `snp doctor` reports an error when the
same event has both a config hook and a hooks-directory script that call a hook manager (git would run both).

## Scripts and tools that call `pre-commit`

Three options, from the most explicit to the least invasive:

* **Call `snp`.** `snp run --all-files` accepts exactly the options of `pre-commit run --all-files`.
* **Keep the name** with `snp self alias install` (see [Installation](installation.md#the-pre-commit-alias)). Every
  `pre-commit …` invocation that resolves to the alias runs snp in the compat brand: pre-commit 4.6.2 to the byte.
* **In Python environments** (tox, nox, `requirements-dev.txt`): depend on `snp-cli` instead of `pre-commit` and call
  `snp`. Never install both into one environment with the alias in it: the first `pre-commit` on `PATH` wins and the
  result depends on installation order.

`entry: pre-commit hazmat …` in a config works without any `pre-commit` executable (snp runs `hazmat` natively). Other
`entry: pre-commit …` hooks need the alias, exactly as they need Python pre-commit today.

## The cache

snp keeps its own cache and never touches pre-commit's:

| | pre-commit | snp |
|---|---|---|
| Location | `$PRE_COMMIT_HOME`, else `$XDG_CACHE_HOME/pre-commit`, else `~/.cache/pre-commit` | `$SNP_HOME`, else `$XDG_CACHE_HOME/snp`, else `~/.cache/snp` (Windows: `%USERPROFILE%\.cache\snp`) |
| Format | SQLite database + directories | versioned, hash-keyed directories, no SQLite |
| Crash log | `pre-commit.log` | `snp.log` (`pre-commit.log` under the `pre-commit` alias) |

`PRE_COMMIT_HOME` does **not** move snp's cache. If `SNP_HOME` points at pre-commit's cache directory snp refuses to
use it (`snp refuses to use pre-commit's cache directory …; set SNP_HOME to another directory`). The first run builds
the hook environments again in snp's cache; `snp install --install-hooks` does that ahead of time.

Running as root (for example in a CI container) against a cache directory owned by another user is refused; set
`SNP_ALLOW_FOREIGN_STORE=1` if that is intended.

## git versions

snp works with every git that pre-commit 4.6.2 works with. Below git 2.31 it prints one warning per run on stderr
(`SNP_ALLOW_OLD_GIT=1` silences it) and uses fallbacks where pre-commit crashes (for example `run --all-files` on
git < 2.31, [D-CRASH-26](divergences.md#d-crash-26)). Below git 2.19 snp refuses to stash unstaged changes, before
touching anything, because git cannot report intent-to-add files there and pre-commit then strands the work in a patch
file ([D-DATA-17](divergences.md#d-data-17)); commit with nothing unstaged, or upgrade git (2.31 or newer recommended).

## Rollback

Back to Python pre-commit, in this order:

1. Reinstall Python pre-commit if you removed it (`pipx install pre-commit`, `uv tool install pre-commit`, …).
2. `snp self alias uninstall` if you created the alias (it prints `Removed …/pre-commit` and restores a
   `pre-commit.snp-backup`).
3. In each repository run `pre-commit install [-t TYPE …]`. pre-commit recognises snp's script and replaces it in place
   (`pre-commit installed at .git/hooks/pre-commit`, no `.legacy`). Alternatively `snp uninstall` first: it removes
   snp's script and restores a foreign `.legacy`.
4. `pre-commit init-templatedir DIR` if you use a template directory; revert `hook.pre-commit.*.command` config
   hooks.
5. In CI, restore `pre-commit/action` or the old commands and **change the cache key again**.
6. `snp clean` removes snp's cache (`Cleaned /home/me/.cache/snp.`). pre-commit's cache was never touched.

Configs need nothing: snp never writes snp-only syntax into `.pre-commit-config.yaml`, and its `autoupdate` and
`migrate-config` output is pre-commit's. While both tools are installed, the last `install` wins.

## Coming from prek or lefthook

* **prek:** `snp install` recognises prek's hook scripts and replaces them in place; prek-only options the script used
  are reported (`Note: the replaced prek hook used prek-only options (…); they are not supported and were dropped`).
  prek recognises snp's script as well, so `prek install` switches back.
* **lefthook:** `snp migrate lefthook` converts `lefthook.yml` into a `.pre-commit-config.yaml` that uses the snp
  extension keys (priorities, groups, `env`, `stage_fixed`, conditions) and a local overlay for `lefthook-local.yml`,
  and reports everything that could not be converted exactly.
