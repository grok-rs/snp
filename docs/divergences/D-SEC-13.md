# D-SEC-13

as root (euid 0), a store owned by a non-root uid is refused unless `SNP_ALLOW_FOREIGN_STORE=1`

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-13.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| a store writable by another user is used as is, including when running as root (tools and environments that user can replace get executed) | as root (euid 0), a store owned by a non-root uid is refused with `An error has occurred: FatalError: snp refuses to run as root from a store owned by uid <n>: <path>; set SNP_ALLOW_FOREIGN_STORE=1 to trust it` (exit 1) unless `SNP_ALLOW_FOREIGN_STORE=1`; other foreign owners only warn (D-DIAG-05) |

## Rationale

pre-commit uses whatever directory `PRE_COMMIT_HOME`/`XDG_CACHE_HOME`/`~/.cache` names and executes the hook
repositories and environments it finds there. When root runs pre-commit against a store that an unprivileged user
owns (a CI cache volume mounted into a container, `sudo` with a preserved `HOME`), that user can replace any
interpreter, `node_modules` entry or clone in the store and gets code execution as root — git refuses the analogous
repository situation since 2.35.2 (`safe.directory`).

snp's `Store::ensure_created` checks the store's owner (ADR-044, ADR-077): owned by the effective uid → used; owned by
another uid → used with one stderr warning (D-DIAG-05), which keeps volume-mounted CI caches working; but when the
effective uid is 0 and the owner is not root, the store is refused with the FatalError above (exit 1) unless
`SNP_ALLOW_FOREIGN_STORE=1` is set, in which case it is used with the warning. A world-writable store without the
sticky bit is refused for every user (symlink planting).

Safe: the refusal happens before anything from the store is executed, names the escape hatch, and affects only the
root-over-user case where the trust boundary is crossed.

## Examples

Needs root and a store owned by another uid, which the differential harness cannot arrange portably; the scenario
records `status: unit`. `crates/snp-store/tests/support/behaviour.rs::foreign_store_as_root_is_refused_unless_allowed`
(posix, runs in the CI container as root) chowns a store to uid 1000 and expects, exit 1:

```text
An error has occurred: FatalError: snp refuses to run as root from a store owned by uid 1000: <store>; set SNP_ALLOW_FOREIGN_STORE=1 to trust it
```

and, re-run with `SNP_ALLOW_FOREIGN_STORE=1`, success with the stderr warning
`[WARNING] the snp store <store> is owned by uid 1000, not by the current user`. pre-commit 4.6.2 uses such a store
without a word.
