# D-CRASH-21

treated as "not rootless"

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-25; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-21.yaml` (WBS "Divergence files", ADR-071).
* **Status:** filled by WP-25. The rule lives in the docker run prefix shared by `language: docker` and
  `language: docker_image` (`crates/snp-lang/src/common/docker.rs`, `is_rootless`); WP-25's tests:
  `crates/snp-lang/tests/group2/docker.rs` (`lang_docker_rootless_probe_failure_is_not_rootless`,
  `lang_docker_rootless_json_rows`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| `docker system info` fails during the rootless probe → `CalledProcessError` exit 3 | treated as "not rootless" |

## Rationale

Before every `docker` / `docker_image` hook run pre-commit 4.6.2 decides whether to pass `-u <uid>:<gid>` to
`docker run` by asking the daemon whether it is rootless (`docker.py:109-128`):

```python
@functools.lru_cache(maxsize=1)
def _is_rootless() -> bool:
    retcode, out, _ = cmd_output_b('docker', 'system', 'info', '--format', '{{ json . }}')
    if retcode != 0:
        return False
    info = json.loads(out)
    ...
```

`cmd_output_b` defaults to `check=True`, so a failing `docker system info` raises `CalledProcessError` before the
`retcode != 0` branch is reached: the branch the author wrote to treat a failing probe as "not rootless" is dead code.
The same holds for output that is not JSON (`json.JSONDecodeError`) or a JSON value without `.get`
(`AttributeError`). The consequence is a crash of the whole run (`An unexpected error has occurred:
CalledProcessError: …`, exit 3), and it happens in situations where the hook would otherwise work or fail with docker's
own, much clearer message: a daemon that answers `docker run` but refuses `system info` (restricted socket proxies,
some rootless/podman setups, `DOCKER_HOST` pointing at a remote that limits the API), or a `docker` CLI shim that does
not implement `system info`.

snp implements the evident intent of the code: a probe that fails, prints something unparsable, or prints JSON of the
wrong shape means "not rootless", and the hook runs with `-u <uid>:<gid>` exactly as it would against a regular
daemon. The probe still runs at the same moment, with the same argv and the same environment, once per process
(pre-commit's `lru_cache`).

The deviation is safe: it only replaces a crash by the behaviour pre-commit itself prescribes for this case. If the
daemon really is unusable, the subsequent `docker run` (and, for `language: docker`, the `docker build`) fails and the
hook reports docker's error; nothing is skipped and no failing outcome turns into success (COMPATIBILITY K5). A
rootless daemon whose probe fails gets `-u <uid>:<gid>`, which is what every pre-commit release before 4.5 passed
unconditionally.

## Examples

A local `docker_image` hook with a fake `docker` on `PATH` whose `system info` exits 1
(`tests/differential/divergences/D-CRASH-21.yaml`):

pre-commit 4.6.2, exit 3:

```
fake docker..............................................................An unexpected error has occurred: CalledProcessError: command: ('/bin/sh', '<work>/tools/docker', 'system', 'info', '--format', '{{ json . }}')
return code: 1
stdout: (none)
stderr:
    Cannot connect to the Docker daemon
Check the log at <store>/pre-commit.log
```

snp, exit 0 (the fake `docker run` prints its arguments):

```
fake docker..............................................................Passed
```

and with `--verbose` the hook output shows `run --rm -u <uid>:<gid> -v <work>:/src:rw,Z --workdir /src alpine:3 true`.
