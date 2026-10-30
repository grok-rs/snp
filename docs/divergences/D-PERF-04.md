# D-PERF-04

skipped when the tagged image exists (SNP_DOCKER_ALWAYS_BUILD=1 restores it)

* **Class:** PERF (COMPATIBILITY §5.6)
* **Owner:** WP-25; the first package of the register row owns this file and
  `tests/differential/divergences/D-PERF-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** filled by WP-25 (ADR-035). The backend plans the rebuild as a `PreStep` with a skip probe
  (`crates/snp-lang/src/langs/docker.rs`); the executor (snp-run, WP-32) runs the probe and skips the step when it
  succeeds. Tests: `crates/snp-lang/tests/group2/docker.rs` (`lang_docker_run_build_is_skipped_when_the_image_exists`,
  `lang_docker_always_build_forces_the_rebuild`), `tc_docker_build_and_run` (real docker, nightly).

## Register entry (COMPATIBILITY §5.6)

| pre-commit 4.6.2 | snp |
|---|---|
| docker hooks run `docker build` every run | skipped when the tagged image exists (`SNP_DOCKER_ALWAYS_BUILD=1` restores it) |

## Rationale

pre-commit builds a `language: docker` hook repository's image at install time (`docker build --tag <tag> --label
PRE_COMMIT --pull .`) and then, because users prune images, rebuilds it before **every** hook run without `--pull`
(`docker.py:159-171`: "Rebuild the docker image in case it has gone missing"). With a warm build cache the rebuild is
a no-op that still costs a docker client start, a build-context upload of the whole hook repository and the daemon's
cache walk, per docker hook and per run — the dominant cost of a warm run that has docker hooks.

snp asks the daemon first: the per-run step is `docker build --tag <tag> --label PRE_COMMIT .` in the hook repository
(pre-commit's argv, cwd and environment) with the skip probe `docker image inspect <tag>`. When the probe succeeds the
image exists and the build is skipped; when it fails (image pruned, daemon error) the build runs exactly as in
pre-commit, and its failure is pre-commit's `CalledProcessError`.

Skipping is equivalent because the inputs of the build cannot have changed since the image was made: the build context
is the immutable checkout of the hook repository at a fixed `rev` (snp's prefixes are never modified after the
install, ADR-025), the per-run build never passes `--pull`, and its output was never shown. The one precondition is
that the tag names *this* repository's image: pre-commit's tag is `pre-commit-<md5(basename(prefix))>`, and because
every snp prefix is named `src` snp hashes the store's repository key instead (ADR-035), so two repositories never
share a tag and the probe cannot find another repository's image. What a skip can miss is a base image that was
re-tagged upstream since the install — which pre-commit's `pull=False` rebuild does not pick up either, since the cached
layers are reused.

`SNP_DOCKER_ALWAYS_BUILD` (set, non-empty, not `0`/`false`) drops the probe and restores pre-commit's unconditional
rebuild, e.g. for local Dockerfiles whose `ADD` of remote URLs should be re-evaluated on every run.

## Examples

A local `docker` hook run twice with a fake `docker` that logs its verbs
(`tests/differential/divergences/D-PERF-04.yaml`). Both tools print the same transcript and exit 0:

```
[INFO] Initializing environment for local.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
fake docker..............................................................Passed
fake docker..............................................................Passed
```

The fake docker's log differs. pre-commit 4.6.2 (recorded with `pc462-py312`):

```
build --pull
build
run
build
run
```

snp:

```
build --pull
image inspect
run
image inspect
run
```

With `SNP_DOCKER_ALWAYS_BUILD=1`, snp's log equals pre-commit's.
