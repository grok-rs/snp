# 01 — pre-commit configuration & manifest schema, validation and YAML semantics

Spec for **snp** ("You Shell Not Pass"). Target: byte-compatible behavior with **pre-commit 4.6.2**
(source at `refs/pre-commit`, identical to the PyPI wheel; verified with `diff -r`).

## 0. Scope, versions, conventions

| Component | Version used as reference | Notes |
|---|---|---|
| pre-commit | 4.6.2 (`pre-commit/setup.cfg:3`) | Experiments were run against a scratch venv with `pre-commit==4.6.2` from PyPI. The installed `~/.local/bin/pre-commit` is **4.1.0**; its differences are listed in §15. |
| cfgv | 3.5.0 (`cfgv/setup.cfg`) | Generic validation engine. Every error message format comes from here. |
| PyYAML | 6.0.3 with libyaml (`yaml.__with_libyaml__ == True`) | pre-commit uses `CSafeLoader` when available, which is always the case for PyPI wheels. |
| identify | 2.6.20 (latest on PyPI) | Supplies `ALL_TAGS` (323 tags), which validate `types`, `types_or` and `exclude_types`. |

Conventions:
- Source citations use paths relative to `refs/`, e.g. `pre-commit/pre_commit/clientlib.py:230`, and `cfgv/cfgv.py:LINE`.
  PyYAML citations point into the installed wheel and are written as `PyYAML yaml/<file>.py:LINE`.
- Text in fenced blocks is **verbatim**. `{x}` means Python `str(x)` and `{x!r}` means Python `repr(x)` (§2.6).
- "warning" means a `logging.WARNING` record on logger `pre_commit`. It prints as `[WARNING] <msg>` on **stdout** (§8.2).
- Unless noted otherwise, every behavior below was checked against 4.6.2 with crafted files. Appendix A lists those fixtures with their exact output.
- The raw fixture files used for the experiments are kept in `scratchpad/tmp-config/*.yaml`, together with the probe scripts in `scratchpad/scripts/` and `scratchpad/probe-*` (Rust probes for `regex`/`fancy-regex`, `libyaml-safer` and `file-identify`). They can be re-run to regenerate golden outputs.

---

## 1. Constants and enumerations

### 1.1 File names and sentinels (`pre-commit/pre_commit/constants.py:5-13`, `clientlib.py:290-291`)

| Constant | Value | Notes |
|---|---|---|
| `CONFIG_FILE` | `.pre-commit-config.yaml` | Default for `-c/--config`. pre-commit never looks for a `.yml` variant. |
| `MANIFEST_FILE` | `.pre-commit-hooks.yaml` | Looked up at the root of a cloned hook repo. |
| `LOCAL` | `local` | Sentinel `repo:` value. Matching is exact and case-sensitive. |
| `META` | `meta` | Sentinel `repo:` value. |
| `DEFAULT` | `default` | Sentinel `language_version`. An explicit `language_version: default` means the same as leaving the key out. |
| `LOCAL_REPO_VERSION` | `'1'` | Store bookkeeping for local repos. Not a config concern. |
| `VERSION` | installed version string (`importlib.metadata.version('pre_commit')`) | Used by `minimum_pre_commit_version` (§5.9). |

### 1.2 Hook types and stages (`clientlib.py:26-39`)

`HOOK_TYPES` in **source order** (this order matters for `--hook-type` choices and help output):
```
commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg
```
`STAGES = (*HOOK_TYPES, 'manual')`, which is 11 values. `manual` is never triggered by a git hook.

Error messages list choices **sorted** (`cfgv.check_one_of` sorts them):
- `default_install_hook_types` (HOOK_TYPES): `commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg`
- `stages` / `default_stages` (STAGES): `commit-msg, manual, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg`

### 1.3 Legacy stage aliases (`clientlib.py:64-72`)

| Legacy | Canonical |
|---|---|
| `commit` | `pre-commit` |
| `merge-commit` | `pre-merge-commit` |
| `push` | `pre-push` |

`transform_stage(s) = _STAGES.get(s, s)`. It is applied:
- to `stages` in manifest hooks, local hooks and meta hooks (StagesMigration), and to config hooks of remote repos (StagesMigrationNoDefault);
- to `default_stages` at the top level;
- to `pre-commit run --hook-stage` (argparse `type=transform_stage` runs **before** the `choices` check, so `--hook-stage commit` is accepted silently; `pre-commit/pre_commit/main.py:84-90`).

It is **not** applied to `default_install_hook_types` or to `-t/--hook-type`. A legacy name there is an error (fixture `c9`).
Using a legacy name produces a deprecation warning (§7) everywhere except the manifest, where the warning is emitted once per repo at clone time (§4.3).

### 1.4 Languages (`pre-commit/pre_commit/all_languages.py:27-50`)

`language_names = sorted(languages)`, which gives 21 names:
```
conda, coursier, dart, docker, docker_image, dotnet, fail, golang, haskell, julia, lua, node, perl, pygrep, python, r, ruby, rust, swift, unsupported, unsupported_script
```
Languages **without an environment** (`ENVIRONMENT_DIR is None`) are `docker_image`, `fail`, `pygrep`, `unsupported` and `unsupported_script` (`languages/*.py`). This set drives the checks in §6.4.

Aliases and removals:

| Written | Effect in 4.6.2 | Where accepted |
|---|---|---|
| `system` | translated to `unsupported` by `_translate_language` (`clientlib.py:194-198`) | `language` of manifest hooks, local hooks and remote-config hook overrides. **Not** accepted as a `default_language_version` key (error, fixture `c14`). **Not** accepted for meta hooks (error, fixture `m3`). |
| `script` | translated to `unsupported_script` | same as `system` |
| `python_venv` | **rejected** (`Expected one of … but got: 'python_venv'`). Removed in 4.0.0 (CHANGELOG). Only `migrate-config` rewrites it to `python` (§10). | – |

The translation happens **before** the `one_of` check during validation and again in `apply_defaults`. Downstream code, and every message that names the language, therefore sees the canonical name. For example, `language: system` produces "…using language `unsupported`…" (fixture R3).

### 1.5 File type tags (`clientlib.py:42-47`)

Valid tags are identify's `ALL_TAGS`. identify 2.6.20 has 323 tags, including `file directory symlink socket executable non-executable text binary`.
**Parity gap (verified):** the user's crate `file-identify` 0.4.0 exposes `file_identify::tags::ALL_TAGS` with 292 tags. The following **31 tags are missing** compared with identify 2.6.20:
```
entitlements gemfile-lock hlsl ipxe jbuilder luau meson-options nushell pipfile-lock piskel poetry-lock pyproject quarto robot scons slnx uv-lock vala wixproj wxi wxl wxs xaml xcconfig xcodebuild xcprivacy xcscheme xcsettings xctestplan xcworkspacedata xliff
```
A config that uses any of these (for example `types: [pyproject]` or `types: [uv-lock]`) is valid for pre-commit and would be **rejected** by snp until `file-identify` catches up. This blocks release.

---

## 2. cfgv: the validation engine (`cfgv/cfgv.py`)

All config and manifest validation is `cfgv.load_from_filename(filename, schema, load_strategy, exc_tp)`. snp must reproduce cfgv's **algorithm, item order and messages** exactly.

### 2.1 `load_from_filename` (`cfgv/cfgv.py:424-450`)

```
with reraise_as(exc_tp):                          # ValidationError -> InvalidConfigError / InvalidManifestError
    if not os.path.isfile(filename):              # follows symlinks; directory or missing -> error
        raise ValidationError(f'{display_filename} is not a file')     # NOTE: no "File ..." context
    with validate_context(f'File {display_filename}'):
        contents = open(filename, encoding='utf-8').read()   # strict UTF-8, universal newlines (\r\n and \r become \n)
            # UnicodeDecodeError -> ValidationError(str(e))
        data = load_strategy(contents)            # ANY Exception -> ValidationError(str(e))
        validate(data, schema)                    # schema.check(data)
        return apply_defaults(data, schema)
```
- `display_filename` is the path as passed: the CLI argument for `validate-*`, and the `--config` value made relative to the git root for run-type commands (§8.4).
- `load_strategy` errors (YAML errors, and also `ValueError`/`KeyError`/`AttributeError` raised by PyYAML constructors) all turn into a leaf message equal to `str(exception)` (§12.8).
- Exceptions raised **during `validate`** that are *not* `ValidationError` (TypeError, ValueError…) escape cfgv and become "An unexpected error has occurred" with exit 3 (§8.3, §16 crash list).

### 2.2 Error object and rendering (`cfgv/cfgv.py:10-23`, `30-45`)

`ValidationError(error_msg, ctx=None)`. `validate_context(msg)` wraps any `ValidationError` raised inside as `ValidationError(inner, ctx=msg)`. `__str__`:
```
out = '\n'
while err.ctx is not None: out += f'==> {err.ctx}\n'; err = err.error_msg
out += f'=====> {err.error_msg}'
```
So the rendered string **starts with `\n`**, has one `==> ctx` line per context from outermost to innermost, ends with `=====> leaf`, and has **no trailing newline**.
`validate-config` prints it with `print(e)`, which adds `\n`. Exact bytes for fixture `local_missing.yaml`:
```

==> File local_missing.yaml
==> At Config()
==> At key: repos
==> At Repository(repo='local')
==> At key: hooks
==> At Hook(id='a')
=====> Missing required key: name
```
(The first line is empty.) If the leaf message itself contains newlines (YAML errors), it is printed as-is after `=====> `.

A nested ValidationError used as a *leaf string* happens when a `load_strategy` raises a ValidationError: cfgv catches it as `Exception` and re-raises `ValidationError(str(e))`. The leaf text then begins with `\n=====> `, which renders as `=====> ` (with a trailing space) followed by `=====> real message` on the next line. See §4.2 and fixture `h2`.

### 2.3 Context strings

| Producer | Context text |
|---|---|
| `load_from_filename` | `File {display_filename}` |
| `Map.check` with `id_key=None` | `At {object_name}()`, e.g. `At Config()`, `At DefaultLanguageVersion()` |
| `Map.check` with `id_key` | `At {object_name}({id_key}={value!r})`. If the key is absent the value prints as `MISSING`, giving `At Repository(repo=MISSING)` and `At Hook(id=MISSING)`. `value!r` is a Python repr of **any** type, e.g. `At Hook(id=1)`, `At Hook(id=[1, 'x'])`, `At Hook(id=datetime.date(2020, 1, 1))`, `At Hook(id=b'hi')`, `At Hook(id=nan)`, `At Hook(id="it's")` |
| `_check_optional` (Optional, OptionalNoDefault, Required, *Recurse, Conditional*) | `At key: {key}` |
| `check_array` | `At index {i}` (0-based). It is used **only** for scalar arrays. `Array` of maps (repos, hooks, manifest) adds **no** index context. |
| `StagesMigrationNoDefault.check`, `LanguageMigration.check` | `At key: {key}`, added manually in 4.6.2 (§15) |

### 2.4 Item kinds and semantics (`cfgv/cfgv.py:48-224`)

| Item | check(dct) | apply_default(dct) |
|---|---|---|
| `Required(key, fn)` | missing → `Missing required key: {key}`; otherwise `fn(value)` inside `At key:` | no-op |
| `RequiredRecurse(key, schema)` | like Required, with `fn = validate(v, schema)` | `dct[key] = apply_defaults(dct[key], schema)` |
| `Optional(key, fn, default)` | if present: `fn(value)` inside `At key:` | `dct.setdefault(key, default)` (shared default objects; never mutate them) |
| `OptionalRecurse(key, schema, default)` | if present: validate recursively inside `At key:` | if absent set default, then always apply nested defaults |
| `OptionalNoDefault(key, fn)` | if present: `fn` inside `At key:` | no-op |
| `Conditional(key, fn, condition_key, condition_value, ensure_absent=False)` | if `dct.get(condition_key, MISSING) == condition_value`: **Required** check. Elif `condition_key in dct and ensure_absent and key in dct`: error (below). | no-op |
| `ConditionalOptional(key, fn, default, ck, cv, ensure_absent=False)` | same, with an **Optional** check | if the condition matches: `setdefault` |
| `ConditionalRecurse(key, schema, ck, cv, ensure_absent=False)` | Conditional + recursive validate | if the condition matches: apply nested defaults |
| `NoAdditionalKeys(keys)` | `extra = sorted(set(dct) - set(keys))`; if any: `Additional keys found: {', '.join(str(x) for x in extra)}.  Only these keys are allowed: {', '.join(str(x) for x in keys)}` (keys in **declaration order**, which is sorted language names here) | no-op |
| `WarnAdditionalKeys(keys, callback)` | `extra = sorted(set(dct) - set(keys))`; if any: `callback(extra, keys, dct)` | no-op |

Ensure-absent error (`cfgv/cfgv.py:109-120`):
```
Expected {key} to be absent when {condition_key} {explanation}, found {key}: {value!r}
```
`explanation` is `condition_value.describe_opposite()`: for `NotIn(*values)` it is `is any of {values!r}` (a **Python tuple repr**, e.g. `('local', 'meta')`). For `In` it is `is not any of {values!r}`, for `Not(v)` it is `is {v!r}`, and for a plain value it is `is not {value!r}`.
`NotIn.__eq__(other)` is `other is not MISSING and other not in values`. A missing condition key never matches.

Containers:
- `Map(object_name, id_key, *items)` (`cfgv/cfgv.py:227-258`). check: if the value is not a `dict`, the message is `Expected a {object_name} map but got a {type(v).__name__}` (note "a int", "a NoneType" — the article is not adjusted). Then each item is checked **in declaration order** inside the context. **The first error aborts**, so warnings from earlier items are still emitted. `apply_defaults` works on a shallow copy and runs each item's `apply_default` in order.
- `Array(of, allow_empty=True)` (`cfgv/cfgv.py:295-314`): `check_array(check_any)(v)`, which gives `Expected array but got {type!r}` (the type name **is quoted**, e.g. `'dict'`). Then `validate(item, of)` for each item, with **no index context**. `allow_empty` is always True in pre-commit, so empty `repos`/`hooks`/manifests are valid.

Check functions (`cfgv/cfgv.py:353-408`):

| fn | Accepts | Message on failure |
|---|---|---|
| `check_any` | everything | – |
| `check_type(tp, typename)` | `isinstance(v, tp)` | `Expected {typename or tp.__name__} got {type(v).__name__}` |
| `check_bool` | Python `bool` only (`1` is rejected) | `Expected bool got int` |
| `check_string` | `str` | `Expected string got float` (the typename is **"string"**, while the actual-type name is Python's: `str` never appears here, but `int float bool NoneType list dict date datetime bytes set tuple` do) |
| `check_type(dict)` (for `ci`) | `dict` | `Expected dict got list` / `Expected dict got NoneType` |
| `check_one_of(possible)` | `v in possible` | `Expected one of {', '.join(str(x) for x in sorted(possible))} but got: {v!r}` |
| `check_regex` | `re.compile(v)` succeeds | `{v!r} is not a valid python regex` |
| `check_array(inner)` | `list` or `tuple` | `Expected array but got {type!r}`. Elements are checked inside `At index {i}`. |
| `check_and(*fns)` | runs all in order; the first failure wins | – |

### 2.5 Python type names used in messages

`NoneType bool int float str bytes list tuple dict set date datetime`. These are the only types PyYAML SafeLoader can produce (§12.3). `!!omap`/`!!pairs` produce a `list` of `tuple`s.

### 2.6 Python `repr` rules that snp must implement

The `{x!r}` placeholders appear in contexts, `but got:`, `found rev:`, regex errors, and warnings about `files`/`exclude`/`rev`/hook ids. They must be bit-exact:
- **str**: wrapped in `'…'`. When the string contains `'` and no `"`, it is wrapped in `"…"` instead. When it contains both, `'` is used and `'` is escaped as `\'`. Escapes: `\\`, `\n`, `\r`, `\t`. Any other non-printable code point (per `str.isprintable`: Unicode categories Cc, Cf, Cs, Co, Cn, Zl, Zp, and Zs except ASCII space) is written as `\xNN` (≤0xff), `\uNNNN` (≤0xffff) or `\UNNNNNNNN`, in lowercase hex. Printable non-ASCII is kept as-is (`'é'`, `'😀'`). Verified: `"it's"`, `'say "hi"'`, `'both \'" q'`, `'back\\slash'`, `'\x07bell'`, `'\x7f'`, `'\x85'`, `'\xa0nbsp'`, `'​'`, `' '`.
  Consequence: the pattern `^a\.txt$` prints as `'^a\\.txt$'` (fixture M2).
- **int**: decimal of arbitrary size (YAML ints can exceed i64: `rev: 123456789012345678901234567890123` → `Expected string got int`).
- **float**: Python `repr(float)`, the shortest round-trip form, with these rules: `1.0` (never `1`), `1.1`, `1e+16`, `1e-05`, `inf`, `-inf`, `nan`, `-0.0`, `123456789.0`. It switches to exponent form when exp < -4 or ≥ 16.
- **bool/None**: `True`, `False`, `None`.
- **list**: `[a, b]`. **tuple**: `('a', 'b')`, with a one-element tuple as `('x',)`. **dict**: `{'a': 1}`. **set**: `{'a'}`. **bytes**: `b'hi'`.
- **date**: `datetime.date(2020, 1, 1)`. **datetime**: `datetime.datetime(2001, 12, 14, 21, 59, 43, 100000, tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=68400)))`. Microseconds and tzinfo are omitted when zero or absent, and UTC prints as `tzinfo=datetime.timezone.utc`. Keeping the original scalar plus a lazily built repr is acceptable; these are corner cases.
- `MISSING` prints as `MISSING`.

`{x}` (str) is used in: `At key: {key}`, `Missing required key: {key}`, `Additional keys found:` lists, `Unexpected key(s)` lists, the `one_of` choices list, `Unexpected key(s) present on {repo}`, and `{hook.id} does not apply…`.

---

## 3. `.pre-commit-config.yaml` schema

Notation for the ordered item tables: **#** is the check order, which also fixes the emission order of warnings and which error wins.

### 3.1 Top level: `CONFIG_SCHEMA = Map('Config', None, …)` (`clientlib.py:496-539`)

| # | Key / item | Kind | Validation | Default after load |
|---|---|---|---|---|
| 1 | `minimum_pre_commit_version` | Optional | `check_string` then `check_min_version` (§5.9). Checked **first** "in case it uses some newer, incompatible feature". | `'0'` |
| 2 | `repos` | **RequiredRecurse** | `Array(CONFIG_REPO_DICT)` (§3.3). Missing → `Missing required key: repos` | – |
| 3 | `default_install_hook_types` | Optional | `check_array(check_one_of(HOOK_TYPES))`. No stage aliases and no `manual`. | `['pre-commit']` |
| 4 | `default_language_version` | OptionalRecurse | `DEFAULT_LANGUAGE_VERSION` (§3.2) | `{}`, then filled to all 21 languages → `'default'` |
| 5 | `default_stages` | StagesMigration (§5.1) | each item transformed via §1.3, then `one_of(STAGES)` | `STAGES`, all 11 including `manual`, in source order (a list after the transformation pass) |
| 6 | `default_stages` | DeprecatedDefaultStagesWarning (§5.3) | warning only | – |
| 7 | `files` | Optional | `check_string` then `check_regex` | `''` |
| 8 | `exclude` | Optional | `check_string` then `check_regex` | `'^$'` |
| 9 | `fail_fast` | Optional | `check_bool` | `False` |
| 10 | (root) | WarnAdditionalKeys | known keys: `repos, default_install_hook_types, default_language_version, default_stages, files, exclude, fail_fast, minimum_pre_commit_version, ci` | – |
| 11 | `files` | OptionalSensibleRegexAtTop (§5.7) | warnings | – |
| 12 | `exclude` | OptionalSensibleRegexAtTop | warnings | – |
| 13 | `ci` | OptionalNoDefault | `check_type(dict)` → `Expected dict got …`. The content is **not** validated. | (absent stays absent) |

Notes:
- **The top-level list form** (pre-1.0 config) is rejected with `Expected a Config map but got a list`. Users have to run `migrate-config`.
- An empty file, a comment-only file, or `---` alone gives `Expected a Config map but got a NoneType`.
- `default_stages: []` is valid and **means hooks without explicit stages never run** (§6.3). Fixture R5 shows the run exiting 0 with no output.
- `repos: []` is valid.
- Unknown keys at the root and at repo level (e.g. a leftover `sha:`) only warn. A root key named `ci` never warns.
- Duplicate repos and duplicate hook ids are **not** detected (all of them run).

### 3.2 `default_language_version`: `Map('DefaultLanguageVersion', None, NoAdditionalKeys(language_names), *Optional(lang, check_string, 'default') for lang in language_names)` (`clientlib.py:491-495`)

- Keys must be canonical language names. `system`, `script`, `python_venv` and typos produce:
  ```
  Additional keys found: python_venv, system.  Only these keys are allowed: conda, coursier, dart, docker, docker_image, dotnet, fail, golang, haskell, julia, lua, node, perl, pygrep, python, r, ruby, rust, swift, unsupported, unsupported_script
  ```
  The extra keys are sorted; the allowed list is in declaration order, which is already sorted.
- Values must be strings. `python: 3.11` is a float and gives `Expected string got float` under context `At key: python`.
- A non-map value gives `Expected a DefaultLanguageVersion map but got a list`.
- Setting a version for an env-less language (e.g. `unsupported: x`) passes validation, but every hook of that language then fails at run time with the `language_version` error (§6.4, fixture R4).

### 3.3 Repository: `CONFIG_REPO_DICT = Map('Repository', 'repo', …)` (`clientlib.py:465-490`)

| # | Item | Semantics |
|---|---|---|
| 1 | `Required('repo', check_string)` | `Missing required key: repo` with context `At Repository(repo=MISSING)` |
| 2 | `ConditionalRecurse('hooks', Array(CONFIG_HOOK_DICT), 'repo', NotIn('local','meta'))` | remote repos: `hooks` is required |
| 3 | `ConditionalRecurse('hooks', Array(LOCAL_HOOK_DICT), 'repo', 'local')` | local: `hooks` is required |
| 4 | `ConditionalRecurse('hooks', Array(META_HOOK_DICT), 'repo', 'meta')` | meta: `hooks` is required |
| 5 | `WarnMutableRev('rev', check_string, condition_key='repo', condition_value=NotIn('local','meta'), ensure_absent=True)` | remote: `rev` is **required** and must be a string, with a mutable-ref warning (§5.6). local/meta: `rev` must be **absent**. |
| 6 | `WarnAdditionalKeys(('repo','rev','hooks'), warn_unknown_keys_repo)` | `Unexpected key(s) present on {repo}: {k1, k2}` |

Consequences:
- `hooks` is checked **before** `rev`. With an invalid `hooks` and a missing `rev`, the `hooks` error wins.
- For a missing `rev`: `Missing required key: rev` with context `At Repository(repo='https://x/y')`.
- `rev` on local/meta: `Expected rev to be absent when repo is any of ('local', 'meta'), found rev: 'v1'` (the value is a repr, e.g. `found rev: 1.0`).
- A missing `hooks` (any repo kind): `Missing required key: hooks`.
- `hooks: {}` gives `Expected array but got 'dict'`; `hooks: [foo]` gives `Expected a Hook map but got a str`.
- `repo` may be any string, including a local path or `file://` URL. Strings other than exactly `local`/`meta` are treated as remote.
- `rev` types: `rev: 1.10` gives `Expected string got float`, `rev: 0123` gives `… got int` (octal), `rev: 2020-01-01` gives `… got date`, and `rev: ~` gives `… got NoneType`. `rev: 1e3456` is accepted as a **string** (§12.2).
- `sha:` (pre-1.0) is unknown here: it causes a warning plus `Missing required key: rev`. `migrate-config` fixes it.

### 3.4 Hook of a remote repo: `CONFIG_HOOK_DICT = Map('Hook', 'id', …)` (`clientlib.py:439-458`)

| # | Item |
|---|---|
| 1 | `Required('id', check_string)` |
| 2–20 | `OptionalNoDefault(key, fn)` for every manifest key except `id`, `stages` and `language`, in manifest order: `minimum_pre_commit_version` (string + min-version check), `name`, `entry`, `alias`, `files` (string+regex), `exclude` (string+regex), `types`, `types_or`, `exclude_types` (array of known tags), `additional_dependencies` (array of strings), `args` (array of strings), `always_run`, `fail_fast`, `pass_filenames` (bool), `description`, `language_version`, `log_file` (string), `require_serial`, `verbose` (bool) |
| 21 | `StagesMigrationNoDefault('stages', [])` (§5.1). No default; transformed if present. |
| 22 | `LanguageMigration('language', check_one_of(language_names))` (§5.4). Override allowed, aliases translated. |
| 23 | `OptionalSensibleRegexAtHook('files', check_string)` (§5.7) |
| 24 | `OptionalSensibleRegexAtHook('exclude', check_string)` |
| 25 | `DeprecatedStagesWarning('stages')` (§5.2) |

- **No defaults are applied** (except the stage and language transformations). The config dict is merged on top of the manifest (§6.1).
- Every manifest key may be overridden, **including `entry`, `name`, `language`, `minimum_pre_commit_version`**.
- Unknown keys are **not** reported at validation time. They are reported at run time when the hook is built (§6.5).

### 3.5 Local hook: `LOCAL_HOOK_DICT = Map('Hook', 'id', *MANIFEST_HOOK_DICT.items, *_COMMON_HOOK_WARNINGS)` (`clientlib.py:459-464`)

Exactly the manifest schema (§4.1), with defaults applied, followed by the three common warnings (`files` and `exclude` sensible-regex, deprecated stages).
Required keys: `id`, `name`, `entry`, `language`. (The docs also mention `files`/`types`, but the schema does **not** require them; `types` defaults to `[file]`.)

### 3.6 Meta hook: `META_HOOK_DICT = Map('Hook', 'id', …)` (`clientlib.py:376-438`)

| # | Item |
|---|---|
| 1 | `Required('id', check_string)` |
| 2 | `Required('id', check_one_of(('check-hooks-apply','check-useless-excludes','identity')))` → `Expected one of check-hooks-apply, check-useless-excludes, identity but got: 'bogus'` |
| 3 | `Optional('language', check_one_of({'unsupported'}), 'unsupported')` → `Expected one of unsupported but got: 'system'`. **`language: system` is rejected for meta hooks in 4.6.2** (it was the *only* value accepted in 4.1.0; §15). |
| 4 | `NotAllowed('entry')` → `'entry' cannot be overridden` (no `At key:` context; reported at the Hook level) |
| 5… | `ConditionalOptional(key, check_any, value, 'id', hook_id)` for each meta default (table below) |
| … | every `MANIFEST_HOOK_DICT` item, except that `name`, `language` and `entry` become `OptionalNoDefault` (type-checked, no default) |
| last | the 3 common warnings |

Meta defaults (`clientlib.py:376-398`):

| id | name | files | verbose | entry (pre-commit) |
|---|---|---|---|---|
| `check-hooks-apply` | `Check hooks apply to the repository` | `^\.pre\-commit\-config\.yaml$` | (manifest default `False`) | `{shlex.quote(sys.executable)} -m pre_commit.meta_hooks.check_hooks_apply` |
| `check-useless-excludes` | `Check for useless excludes` | `^\.pre\-commit\-config\.yaml$` | `False` | `… -m pre_commit.meta_hooks.check_useless_excludes` |
| `identity` | `identity` | (manifest default `''`) | **`True`** | `… -m pre_commit.meta_hooks.identity` |

The `files` value is `'^' + re.escape('.pre-commit-config.yaml') + '$'`. Python's `re.escape` escapes `-` and `.`, giving the literal `^\.pre\-commit\-config\.yaml$`. It is **always** built from `C.CONFIG_FILE`, even when `-c other.yaml` is used, and it matches only the root-level file.
All other keys take manifest defaults (§4.1): `types: [file]`, `exclude: '^$'`, `stages: []` (later replaced by default_stages), `language: unsupported`, and so on. Users may override `name`, `files`, `exclude`, `types*`, `args`, `stages`, `verbose`, `always_run`, `pass_filenames` and so on, but **not** `entry`, and `language` only as `unsupported`.

### 3.7 `ci:` (pre-commit.ci; informational)

pre-commit only checks that `ci` is a map. pre-commit.ci documents these keys and defaults: `autofix_commit_msg` (`'[pre-commit.ci] auto fixes from pre-commit.com hooks\n\nfor more information, see https://pre-commit.ci\n'`), `autofix_prs: true`, `autoupdate_branch: ''`, `autoupdate_commit_msg: '[pre-commit.ci] pre-commit autoupdate'`, `autoupdate_schedule: weekly` (`weekly|monthly|quarterly`), `skip: []` (hook ids, like `SKIP=`), `submodules: false`. snp must store the `ci` map opaquely and must not warn about it.

### 3.8 Fully defaulted config (what `load_config` returns)

```yaml
minimum_pre_commit_version: '0'
repos: [...]                      # each repo: remote = {repo, rev, hooks:[config-level dicts, no defaults]};
                                  # local = {repo, hooks:[fully defaulted manifest-like dicts]}; meta likewise
default_install_hook_types: [pre-commit]
default_language_version: {conda: default, coursier: default, ..., unsupported_script: default}   # all 21
default_stages: [commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg, manual]
files: ''
exclude: '^$'
fail_fast: false
# ci: present only if given; unknown keys are kept as-is (only warned)
```

---

## 4. `.pre-commit-hooks.yaml` manifest schema

### 4.1 `MANIFEST_HOOK_DICT = Map('Hook', 'id', …)`; `MANIFEST_SCHEMA = Array(MANIFEST_HOOK_DICT)` (`clientlib.py:230-266`)

| # | Key | Kind | Check | Default |
|---|---|---|---|---|
| 1 | `minimum_pre_commit_version` | Optional | string + §5.9 (**first** check) | `'0'` |
| 2 | `id` | Required | string | – |
| 3 | `name` | Required | string | – |
| 4 | `entry` | Required | string | – |
| 5 | `language` | LanguageMigrationRequired | aliases translated, then `one_of(language_names)`. Missing → `Missing required key: language` | – |
| 6 | `alias` | Optional | string | `''` |
| 7 | `files` | Optional | string + Python regex | `''` |
| 8 | `exclude` | Optional | string + Python regex | `'^$'` |
| 9 | `types` | Optional | array of known tags (§1.5) | `['file']` |
| 10 | `types_or` | Optional | array of known tags | `[]` |
| 11 | `exclude_types` | Optional | array of known tags | `[]` |
| 12 | `additional_dependencies` | Optional | array of strings | `[]` |
| 13 | `args` | Optional | array of strings | `[]` |
| 14 | `always_run` | Optional | bool | `False` |
| 15 | `fail_fast` | Optional | bool | `False` |
| 16 | `pass_filenames` | Optional | bool | `True` |
| 17 | `description` | Optional | string | `''` |
| 18 | `language_version` | Optional | string | `'default'` |
| 19 | `log_file` | Optional | string | `''` |
| 20 | `require_serial` | Optional | bool | `False` |
| 21 | `stages` | StagesMigration | array; items transformed then `one_of(STAGES)` | `[]` (empty means "use default_stages", §6.3) |
| 22 | `verbose` | Optional | bool | `False` |

- The manifest top level must be a list (`Expected array but got 'str'` / `'NoneType'` for an empty file). `[]` is valid.
- **No warnings at all** are produced by manifest validation: no unknown-key warnings, no regex sensibility warnings, no stage deprecation warnings (the stage warning is emitted on clone, §4.3). Unknown keys surface only at hook build time (§6.5).
- Duplicate ids are accepted. When building `by_id` the **last one wins** (`repository.py:178`).
- `minimum_pre_commit_version: 3.2.0` unquoted is a string (two dots). `3.2` unquoted is a float and gives an error.

### 4.2 Forward-compat guard (`clientlib.py:273-287`)

`load_manifest` uses `_load_manifest_forward_compat`: if the YAML top level is a **dict**, it calls `check_min_version('5')`, which reserves the map-form manifest for pre-commit 5. The `ValidationError` raised inside the load strategy gets stringified (§2.2), so the exact output is:
```

==> File h2.yaml
=====> 
=====> pre-commit version 5 is required but version 4.6.2 is installed.  Perhaps run `pip install --upgrade pre-commit`.
```
Note the trailing space after the first `=====>`. `validate-manifest` exits 1.

### 4.3 Clone-time deprecated-stage warning (`clientlib.py:75-109`, `store.py:173`)

Runs only once, right after a repo is cloned into the store (not on cache hits). It loads the manifest with `MINIMAL_MANIFEST_SCHEMA = Array(Map('Hook','id', Required('id', check_string), Optional('stages', check_array(check_string), [])))` and plain `yaml_load`, so a dict manifest fails silently. Any load error → silently return. Legacy names are collected **in order of first appearance, deduplicated** across all hooks:
```
repo `{repo}` uses deprecated stage names ({", ".join(legacy)}) which will be removed in a future version.  Hint: often `pre-commit autoupdate --repo {shlex.quote(repo)}` will fix this.  if it does not -- consider reporting an issue to that repo.
```
Here `{repo}` is the original `repo:` string, and the `--repo` argument is shell-quoted with POSIX `shlex.quote`: an empty string becomes `''`; a string made only of ASCII `[A-Za-z0-9_@%+=:,./-]` is unchanged; anything else is wrapped in `'…'` with each `'` written as `'"'"'`.
Verified output (fixture R6): `uses deprecated stage names (commit, push, merge-commit)`.

---
## 5. pre-commit-specific validators (exact semantics)

### 5.1 `StagesMigrationNoDefault` / `StagesMigration` (`clientlib.py:112-139`)
- `check(dct)`: if the key is absent, do nothing. Otherwise, **inside `At key: {key}`**: `check_array(check_any)(val)`, which gives `Expected array but got 'str'` for `stages: commit`. Then `val2 = [transform_stage(v) for v in val]` followed by `check_array(check_one_of(STAGES))(val2)`, which reports errors under `At index {i}` with the **transformed** value in `but got:`.
- `apply_default`: `StagesMigrationNoDefault` transforms only when the key is present. `StagesMigration` first does `setdefault(key, default)` and then transforms.
- `transform_stage` does a dict lookup on the raw item. An **unhashable** item (e.g. `stages: [[a]]`) raises `TypeError: unhashable type: 'list'`, which pre-commit reports as an unexpected error, exit 3 (§16).
- Non-string hashable items pass through and fail the `one_of` check, e.g. `but got: 1`.

### 5.2 `DeprecatedStagesWarning('stages')` (hooks; `clientlib.py:142-165`)
Runs after all other checks on that hook. If `stages` is present: `check_array(check_any)`, then `legacy = [s for s in val if s in _STAGES]`, which keeps order and **duplicates**. If any:
```
hook id `{dct["id"]}` uses deprecated stage names ({", ".join(legacy)}) which will be removed in a future version.  run: `pre-commit migrate-config` to automatically fix this.
```

### 5.3 `DeprecatedDefaultStagesWarning('default_stages')` (`clientlib.py:168-191`)
```
top-level `default_stages` uses deprecated stage names ({", ".join(legacy)}) which will be removed in a future version.  run: `pre-commit migrate-config` to automatically fix this.
```

### 5.4 `LanguageMigration` / `LanguageMigrationRequired` (`clientlib.py:194-227`)
- `check`: Required variant: if missing → `Missing required key: language` (no `At key:` context). If present, **inside `At key: language`**: `check_one_of(language_names)(_translate_language(value))`.
- `_translate_language` is a dict lookup, so an unhashable value (e.g. `language: [x]`) gives a TypeError, exit 3 (§16).
- `apply_default`: translate if present.

### 5.5 `NotAllowed('entry')` (meta only; `clientlib.py:401-404`)
If the key is present: `'entry' cannot be overridden`, with the message formatted from `{key!r}`.

### 5.6 `WarnMutableRev` (`clientlib.py:294-310`)
Runs the Conditional check first (§3.3). Then, if `rev` is present on a remote repo (a valid string by then), it warns when `'.' not in rev and not re.match(r'^[a-fA-F0-9]+$', rev)`:
```
The 'rev' field of repo {repo!r} appears to be a mutable reference (moving tag / branch).  Mutable references are never updated after first install and are not supported.  See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.  Hint: `pre-commit autoupdate` often fixes this.
```
(`'rev'` is `{self.key!r}`.) Examples:
- OK, no warning: `v0.12.4`, `b27f281`, a full 40-hex SHA, `19.10b0`, `4.3.21-2`, `deadbeef`, `abc`.
- Warns: `''`, `HEAD`, `stable`, `master`, `main`, `v1`, `some_branch_name`.

Python `$` also matches before a trailing `\n`, so `'abc\n'` counts as hex. This is irrelevant in practice.

### 5.7 `OptionalSensibleRegexAtHook` / `OptionalSensibleRegexAtTop` (`clientlib.py:313-347`)
First `check_string` on the value if present (an OptionalNoDefault check). Then, on `dct.get(key, '')`:
1. If the substring `/*` is present:
   - hook: ``The {key!r} field in hook {dct.get("id")!r} is a regex, not a glob -- matching '/*' probably isn't what you want here``
   - top: ``The top-level {key!r} field is a regex, not a glob -- matching '/*' probably isn't what you want here``
2. For each `fwd_slash_re` in this order: `[\\/]`, `[\/]`, `[/\\]` (raw strings of 5, 4 and 5 characters), **independently**, if it is a substring:
   - hook: `pre-commit normalizes slashes in the {key!r} field in hook {id!r} to forward slashes, so you can use / instead of {fwd_slash_re}`
   - top: `pre-commit normalizes the slashes in the top-level {key!r} field to forward slashes, so you can use / instead of {fwd_slash_re}`

   Note the wording differs: "normalizes slashes in the" for hooks versus "normalizes the slashes in the top-level" for the top level.

These apply to remote-config hooks, local hooks, meta hooks and the top level, but **not** to manifests.

### 5.8 `check_type_tag` (`clientlib.py:42-47`)
```
Type tag {tag!r} is not recognized.  Try upgrading identify and pre-commit?
```
This runs under `At key: types` / `At index i`. A non-string hashable tag fails the same way, e.g. `Type tag 1 is not recognized…`. An unhashable tag (a list) gives a TypeError, exit 3.

### 5.9 `minimum_pre_commit_version` (`clientlib.py:50-61`)
`parse_version(s) = tuple(int(p) for p in s.split('.'))`, which is Python `int()` semantics per component:
- Accepted: surrounding whitespace (`' 4'`, `'\t3\n'`), a leading `+` or `-`, `_` between digits (`'4_0'` → 40), leading zeros, and **Unicode decimal digits** (`'٤'` → 4).
- Rejected by raising `ValueError` (**not** a ValidationError): `''`, `'4.'`, `'.4'`, `'4..6'`, `'1e3'`, `'0x10'`, `'abc'`, `'4.6.3rc1'`. pre-commit then crashes with `An unexpected error has occurred: ValueError: invalid literal for int() with base 10: 'abc'`, exit **3** (fixtures c3, c6, c7).
- Comparison is a lexicographic tuple comparison: `(4,6) < (4,6,2)` and `(4,6,2,0) > (4,6,2)`. If `parse(required) > parse(VERSION)`:
  ```
  pre-commit version {version} is required but version {C.VERSION} is installed.  Perhaps run `pip install --upgrade pre-commit`.
  ```
- It is checked first at top level, first in manifest hooks and local hooks, and after `id` in remote-config hooks. A failing top-level check therefore masks every other error (fixture c8).

---

## 6. Building hooks: merge semantics and precedence (`repository.py:109-237`, `hook.py`)

### 6.1 Remote repos (`_cloned_repository_hooks`, `repository.py:171-200`)
1. Clone `(repo, rev)`, load the manifest with `load_manifest` (validated, defaults applied) and build `by_id = {h['id']: h}`, where the last duplicate wins.
2. For **each** config hook, in order: if `id` is not in `by_id`, log at ERROR level and `exit(1)` (SystemExit, not caught by the error handler, so the exit code is 1):
   ```
   `{hook["id"]}` is not present in repository {repo}.  Typo? Perhaps it is introduced in a newer version?  Often `pre-commit autoupdate` fixes this.
   ```
   This check runs for all hooks of the repo **before** any of them is built. Repos earlier in the list have already been processed and have printed their warnings.
3. `merged = dict(manifest_hook); merged.update(config_hook)` is a **shallow, whole-key override**: config `args` *replace* manifest `args` and are not appended. The same applies to `additional_dependencies`, `types` and every other key.
4. Then `_hook(...)` resolution (§6.3, §6.4) runs, followed by `Hook.create(repo_url, prefix, merged)` (§6.5).

### 6.2 local and meta repos (`_non_cloned_repository_hooks`, `repository.py:147-168`)
The hook dict is already fully defaulted by `LOCAL_HOOK_DICT`/`META_HOOK_DICT`. Only `_hook(hook)` resolution runs. `src` is `'local'` or `'meta'`.

### 6.3 Resolution rules inside `_hook` (`repository.py:109-144`), in this order
1. **language_version**: if `== 'default'`, use `root.default_language_version[language]`, where `language` is the final merged and translated value, so a config `language:` override changes which key is used. If it is still `'default'`, use `languages[language].get_default_version()` (language-specific; see the languages spec).
   Precedence: config hook value → manifest value → `default_language_version[lang]` → language default. A literal `default` at any level counts as "unset".
2. **stages**: `if not ret['stages']: ret['stages'] = root['default_stages']`. An **empty list at any level counts as unset**. A config `stages: []` overrides a non-empty manifest `stages` and then falls back to `default_stages`, not to the manifest value (verified, fixture R7). `default_stages` defaults to all 11 stages, `manual` included.
3. **Env-less languages** (§1.4): see §6.4.

Precedence summary (remote hook): `config hook key` > `manifest key (with manifest defaults)`, except for `language_version` and `stages`, which have the extra fallbacks above. Top-level `files`/`exclude` are **not** merged into hooks. They are a separate, first-stage filter: `Classifier.from_config` applies the top-level `files`/`exclude` to the candidate files, then each hook's `files`/`exclude`/`types`/`types_or`/`exclude_types` (`commands/run.py:60-126`).

### 6.4 Env-less language guard (`repository.py:126-142`)
If the language has no environment (`docker_image`, `fail`, `pygrep`, `unsupported`, `unsupported_script`):
- if `language_version != 'default'` after resolution → ERROR and `exit(1)`:
  ```
  The hook `{id}` specifies `language_version` but is using language `{lang}` which does not install an environment.  Perhaps you meant to use a specific language?
  ```
- if `additional_dependencies` is non-empty → ERROR and `exit(1)`:
  ```
  The hook `{id}` specifies `additional_dependencies` but is using language `{lang}` which does not install an environment.  Perhaps you meant to use a specific language?
  ```
`{lang}` is the translated name (`system` shows as `unsupported`). A top-level `default_language_version: {unsupported: x}` therefore breaks every `system` hook.

### 6.5 Unknown-key warning at hook build (`hook.py:49-57`)
`_KEYS` is the set of Hook fields minus `src` and `prefix`: `id name entry language alias files exclude types types_or exclude_types additional_dependencies args always_run fail_fast pass_filenames description language_version log_file minimum_pre_commit_version require_serial stages verbose` (22 keys). Any other key in the merged dict (from the manifest **or** the config) gives:
```
Unexpected key(s) present on {src} => {dct["id"]}: {", ".join(sorted(extra_keys))}
```
`{src}` is the repo URL, `local` or `meta`. This is emitted every time hooks are built (`run`, `install-hooks`, `gc`… and inside the meta-hook subprocess). Example: `Unexpected key(s) present on local => echo1: foo, zzz`.

### 6.6 The Hook record (`hook.py:13-37`)
Field order: `src, prefix, id, name, entry, language, alias, files, exclude, types, types_or, exclude_types, additional_dependencies, args, always_run, fail_fast, pass_filenames, description, language_version, log_file, minimum_pre_commit_version, require_serial, stages, verbose`. `install_key = (prefix, language, language_version, tuple(additional_dependencies))`.

### 6.7 Runtime semantics of the keys (brief; details live in the run/language specs)
| Key | Runtime meaning |
|---|---|
| `id` | Selector for `pre-commit run <hook>` and for `SKIP=` (comma-separated, items stripped). Duplicates allowed. |
| `alias` | Extra selector for `run <alias>` and `SKIP`. `''` means none. |
| `name` | Status-line label. CJK width is counted as 2. |
| `entry` | Command prefix. It is shlex-split by language runners. |
| `args` | Appended after `entry`, before filenames. |
| `files` / `exclude` | Python `re.search` on forward-slash relative paths (§13). |
| `types` (AND), `types_or` (OR, ignored if empty), `exclude_types` | Tag filters: `tags >= types and (not types_or or tags & types_or) and not tags & exclude_types` (`commands/run.py:81-98`). |
| `always_run` | Run even with zero matching files. |
| `pass_filenames` | `false` means no filenames are appended. |
| `require_serial` | Disable xargs parallel partitioning. |
| `fail_fast` | After **this** hook fails, stop. Global stop condition: `config.fail_fast or hook.fail_fast or --fail-fast` evaluated on the failing hook (`commands/run.py:301`). |
| `verbose` | Print the output even on success, plus a `- duration:` line. |
| `log_file` | Append the hook output to this file when output is printed. |
| `stages` | The hook runs only if `--hook-stage` (default `pre-commit`) ∈ stages. |
| `minimum_pre_commit_version`, `description` | Validation and metadata only. |
| `language_version`, `additional_dependencies` | Environment identity (install_key). |

---

## 7. Message catalogue (config/manifest related)

Level → print prefix (§8.2). "When" values: **V** = during schema validation (every `load_config`/`load_manifest` call, including `validate-config`, `run`, `install`, `autoupdate`, `gc` and meta hooks), **B** = hook building (run/install-hooks/…), **C** = first clone of a repo.

| # | Level | When | Text (verbatim template) | Source |
|---|---|---|---|---|
| W1 | WARNING | V | `Unexpected key(s) present at root: {", ".join(sorted extra)}` | `clientlib.py:363` |
| W2 | WARNING | V | `Unexpected key(s) present on {repo}: {", ".join(sorted extra)}` | `clientlib.py:371-373` |
| W3 | WARNING | V | ``The 'rev' field of repo {repo!r} appears to be a mutable reference (moving tag / branch).  Mutable references are never updated after first install and are not supported.  See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.  Hint: `pre-commit autoupdate` often fixes this.`` | `clientlib.py:302-310` |
| W4 | WARNING | V | ``The {key!r} field in hook {id!r} is a regex, not a glob -- matching '/*' probably isn't what you want here`` | `clientlib.py:318-322` |
| W5 | WARNING | V | `pre-commit normalizes slashes in the {key!r} field in hook {id!r} to forward slashes, so you can use / instead of {fwd_slash_re}` | `clientlib.py:325-329` |
| W6 | WARNING | V | ``The top-level {key!r} field is a regex, not a glob -- matching '/*' probably isn't what you want here`` | `clientlib.py:337-340` |
| W7 | WARNING | V | `pre-commit normalizes the slashes in the top-level {key!r} field to forward slashes, so you can use / instead of {fwd_slash_re}` | `clientlib.py:343-347` |
| W8 | WARNING | V | ``hook id `{id}` uses deprecated stage names ({list}) which will be removed in a future version.  run: `pre-commit migrate-config` to automatically fix this.`` | `clientlib.py:154-159` |
| W9 | WARNING | V | ``top-level `default_stages` uses deprecated stage names ({list}) which will be removed in a future version.  run: `pre-commit migrate-config` to automatically fix this.`` | `clientlib.py:180-185` |
| W10 | WARNING | C | ``repo `{repo}` uses deprecated stage names ({list}) which will be removed in a future version.  Hint: often `pre-commit autoupdate --repo {shlex.quote(repo)}` will fix this.  if it does not -- consider reporting an issue to that repo.`` | `clientlib.py:102-109` |
| W11 | WARNING | B | `Unexpected key(s) present on {src} => {id}: {sorted keys}` | `hook.py:53-56` |
| E1 | ERROR + exit 1 | B | `` `{id}` is not present in repository {repo}.  Typo? Perhaps it is introduced in a newer version?  Often `pre-commit autoupdate` fixes this. `` (the text starts with a backtick and ends with `this.`) | `repository.py:182-187` |
| E2 | ERROR + exit 1 | B | ``The hook `{id}` specifies `language_version` but is using language `{lang}` which does not install an environment.  Perhaps you meant to use a specific language?`` | `repository.py:128-134` |
| E3 | ERROR + exit 1 | B | ``The hook `{id}` specifies `additional_dependencies` but is using language `{lang}` which does not install an environment.  Perhaps you meant to use a specific language?`` | `repository.py:136-142` |
| V* | ValidationError leaves | V | `Missing required key: {key}` · `Expected {type} got {type}` · `Expected array but got '{type}'` · `Expected a {Obj} map but got a {type}` · `Expected one of {choices} but got: {v!r}` · `{v!r} is not a valid python regex` · `Additional keys found: …  Only these keys are allowed: …` · `Expected rev to be absent when repo is any of ('local', 'meta'), found rev: {v!r}` · `'entry' cannot be overridden` · `Type tag {t!r} is not recognized.  Try upgrading identify and pre-commit?` · ``pre-commit version {v} is required but version {V} is installed.  Perhaps run `pip install --upgrade pre-commit`.`` · `{path} is not a file` · YAML/decoding texts (§12.8) | cfgv / clientlib |

Related messages in other commands (for completeness; owned by other specs):
- `run`, config not staged (logged at ERROR, two lines, exit 1; `commands/run.py:353-358`):
  ```
  [ERROR] Your pre-commit configuration is unstaged.
  `git add .pre-commit-config.yaml` to fix this.
  ```
- `hook-impl`, missing config (`commands/hook_impl.py:49-67`). With `--skip-on-missing-config` or the env `PRE_COMMIT_ALLOW_NO_CONFIG` set to a non-empty value it prints the following, with exit = the legacy hook's retv:
  ```
  `.pre-commit-config.yaml` config file not found. Skipping `pre-commit`.
  ```
  Otherwise it prints the following and exits 1:
  ```
  No .pre-commit-config.yaml file was found
  - To temporarily silence this, run `PRE_COMMIT_ALLOW_NO_CONFIG=1 git ...`
  - To permanently silence this, install pre-commit with the --allow-missing-config option
  - To uninstall pre-commit run `pre-commit uninstall`
  ```
- `install`/`init-templatedir` hook types: `-t` given → use it. Otherwise `load_config(...)['default_install_hook_types']`, and if **InvalidConfigError → silently `['pre-commit']`**. Validation warnings are still printed (`commands/install_uninstall.py:35-44`).

---

## 8. Output, streams, colors and exit codes

### 8.1 `validate-config [filenames ...]` / `validate-manifest [filenames ...]` (`commands/validate_*.py`, `main.py:340-348,439-442`)
- `nargs='*'` with **no default**: **with no filenames nothing is validated, there is no output, and the exit code is 0** (verified). There is no implicit `.pre-commit-config.yaml`.
- Files are processed in argument order. For each invalid file the rendered ValidationError (§2.2) is printed with `print(e)`, i.e. to stdout followed by `\n`, and processing continues. The exit code is 1 if any file was invalid, else 0. Files that are valid but have warnings still exit 0.
- Neither command needs a git repo (`COMMANDS_NO_GIT`, `main.py:44-47`), and neither changes directory, so paths resolve relative to the CWD.
- A non-ValidationError exception (§16) aborts the whole command: `An unexpected error has occurred: {ExcType}: {msg}` + `Check the log at {store}/pre-commit.log` and exit 3. Later files are **not** processed.
- Options: `-h`, `--color {auto,always,never}`. Help text (verbatim, argparse with 80 columns):
  ```
  usage: pre-commit validate-config [-h] [--color {auto,always,never}]
                                    [filenames ...]

  positional arguments:
    filenames

  options:
    -h, --help            show this help message and exit
    --color {auto,always,never}
                          Whether to use color in output. Defaults to `auto`.
  ```
  Argparse collapses the double space after the period in "Whether to use color in output.  Defaults to `%(default)s`." The default shown is `PRE_COMMIT_COLOR` if set, else `auto`. An invalid value is an argparse error with exit 2: `pre-commit validate-config: error: argument --color: invalid use_color value: 'bogus'`.

### 8.2 Log lines (`logging_handler.py:12-31`, `color.py:59-109`, `output.py`)
- Format: `{level_msg} {message}\n`, where `level_msg` is `[WARNING]`, `[ERROR]` or `[INFO]`, written to **stdout** (`sys.stdout.buffer`) and **flushed immediately**.
- With color: `[WARNING]` → `\x1b[43;30m[WARNING]\x1b[m`, `[ERROR]` → `\x1b[41m[ERROR]\x1b[m`, `[INFO]` → `[INFO]\x1b[m` (color `''` but the reset is still appended).
- Color decision: `--color` (default from env `PRE_COMMIT_COLOR`, else `auto`). `auto` means `sys.stderr.isatty()` (**stderr**, even though output goes to stdout) and the terminal supports color (on Windows, enabling VT mode on stderr must succeed) and `TERM != 'dumb'`.
- Python-side buffering artifact: `print(e)` goes through the text-layer `sys.stdout`, which is **block-buffered when stdout is not a TTY**, while warnings are flushed immediately. When piped, the validation-error blocks therefore appear **after** later warnings and even after a crash message. On a TTY the order is chronological. **snp should always emit in chronological order** (a deliberate, harmless deviation; see Open questions).

### 8.3 Config errors in other commands (`error_handler.py:19-81`)
In `run`, `install`, `install-hooks`, `autoupdate`, `migrate-config`, `gc` and so on, an `InvalidConfigError`/`InvalidManifestError` (a FatalError) prints:
```
An error has occurred: InvalidConfigError: 
==> File .pre-commit-config.yaml
==> At Config()
...
=====> <leaf>
Check the log at /home/u/.cache/pre-commit/pre-commit.log
```
The first line ends with `InvalidConfigError: ` (trailing space), because the rendered error starts with `\n`. Exit code **1**.
Other exceptions print `An unexpected error has occurred: {Type}: {msg}` with exit **3**, and KeyboardInterrupt prints `Interrupted (^C): KeyboardInterrupt: ` with exit **130**. The log file `{store}/pre-commit.log` is overwritten with version info and a traceback. If the store dir is not writable, the message is `Failed to write to log at {path}` and the log is written to stdout. Store dir: `$PRE_COMMIT_HOME`, else `$XDG_CACHE_HOME/pre-commit`, else `~/.cache/pre-commit`, then `realpath` (`store.py:25-36`).

### 8.4 Config path resolution for git commands (`main.py:175-199`)
If `--config` exists relative to the CWD, it is made absolute. Then `chdir(git toplevel)` and `config = os.path.relpath(config)`. A default `.pre-commit-config.yaml` that does not exist in a subdirectory CWD therefore resolves to the **root** config. `-c` paths outside the repo become `../…`. This display path is what appears in `==> File …`.

---

## 9. Meta hooks: exact behavior

All three are `language: unsupported` hooks whose `entry` runs a Python module in a subprocess. Their output is captured (stdout+stderr combined) and shown by `run` only on failure or when verbose. Filenames matching the hook's `files` are appended to `args`, and argparse treats **all** of them as config filenames. With no positional args they default to `[.pre-commit-config.yaml]`.

### 9.1 `check-hooks-apply` (`meta_hooks/check_hooks_apply.py:14-39`)
For each filename (config path): `config = load_config(path)`, which re-emits validation warnings (§9.4). Then:
```
classifier = Classifier.from_config(git ls-files -z --deduplicate, config.files, config.exclude)   # top-level filter, then keep os.path.lexists
for hook in all_hooks(config, Store()):          # clones remote repos if needed; merges manifests; may emit E1–E3/W11
    if hook.always_run or hook.language == 'fail': continue
    if no file in classifier.filenames_for_hook(hook): print(f'{hook.id} does not apply to this repository'); retv = 1
```
The exit code is the OR over files. The meta hooks themselves are included in the check: `check-hooks-apply` applies only if `.pre-commit-config.yaml` is tracked.
Output example (fixture M1): `nofiles does not apply to this repository`.

### 9.2 `check-useless-excludes` (`meta_hooks/check_useless_excludes.py:17-79`)
```
def exclude_matches_any(filenames, include, exclude):
    if exclude == '^$': return True                       # default exclude is never "useless"
    for f in filenames: if re.search(include, f) and re.search(exclude, f): return True
    return False

filenames = git ls-files -z --deduplicate                 # ALL tracked files
classifier = Classifier.from_config(filenames, config.files, config.exclude)
if not exclude_matches_any(filenames, '', config.exclude):          # global exclude vs ALL files (top-level files NOT applied)
    print(f'The global exclude pattern {exclude!r} does not match any files'); retv = 1
for repo in config.repos: for hook in repo.hooks:          # CONFIG-LEVEL dicts; remote hooks are NOT merged with the manifest
    hook.setdefault('types', [])                          # remote config hooks: types default [] (not [file])
    hook = apply_defaults(hook, MANIFEST_HOOK_DICT)       # other missing keys: manifest defaults ('' / '^$' / [] ...)
    names = classifier.by_types(classifier.filenames, hook.types, hook.types_or, hook.exclude_types)
    if not exclude_matches_any(names, hook.files, hook.exclude):
        print(f'The exclude pattern {exclude!r} for {hook["id"]} does not match any files'); retv = 1
```
Quirks to preserve:
- For **remote** hooks only the config's own `files`/`types` are used, never the manifest's.
- Local and meta hooks already carry defaults (`types: [file]`).
- `{exclude!r}` is a Python repr, so `^a\.txt$` prints as `'^a\\.txt$'`.

Output example (fixture M2):
```
The global exclude pattern '^nomatch/' does not match any files
The exclude pattern '^zzz' for uselessex does not match any files
The exclude pattern '^a\\.txt$' for typed does not match any files
```

### 9.3 `identity` (`meta_hooks/identity.py:9-13`)
Writes each argv element (hook `args` followed by filenames) on its own line to stdout and exits 0. It defaults to `verbose: true`, so its output is always shown:
```
identity.................................................................Passed
- hook id: identity
- duration: 0.03s

--foo
bar
a.txt

```

### 9.4 Warnings inside meta-hook output
The meta-hook subprocess installs **no** pre-commit logging handler. Python's last-resort handler therefore prints WARNING (and higher) records to **stderr as the bare message**, with no `[WARNING] ` prefix and no color. They appear in the captured hook output (fixture M3):
```
Check hooks apply to the repository......................................Failed
- hook id: check-hooks-apply
- exit code: 1

Unexpected key(s) present at root: bogus_root
nofiles does not apply to this repository
```
If snp runs meta hooks in-process (recommended), it must reproduce this: capture the warnings from its config reload into the hook's output buffer as bare lines.

### 9.5 Invocation notes
- `files` defaults to `^\.pre\-commit\-config\.yaml$`. On a normal commit that does not touch the config file, the two check hooks show `(no files to check)Skipped`.
- With `-c custom.yaml`, the meta hooks still receive and load `.pre-commit-config.yaml`, not `custom.yaml`. This is a pre-commit quirk.
- Because `pass_filenames` is true and argparse uses positional `filenames`, user `args` are treated as additional config paths.

---

## 10. `migrate-config` (`commands/migrate_config.py`)

CLI: `pre-commit migrate-config [-h] [--color …] [-c CONFIG]`. It needs a git repo (it is not in `COMMANDS_NO_GIT`). Outside a repo it prints `An error has occurred: FatalError: git failed. Is it installed, and are you in a Git repository directory?` and exits 1. `autoupdate` calls it first with `quiet=True` (`commands/autoupdate.py:170`).

Algorithm (`migrate_config(config_file, quiet=False)`, lines 114-135):
1. `orig = open(config_file).read()`. This uses the **locale encoding**, not explicitly UTF-8, with universal newlines.
   - Missing file: `An unexpected error has occurred: FileNotFoundError: [Errno 2] No such file or directory: '.pre-commit-config.yaml'`, exit 3.
   - Undecodable file: `UnicodeDecodeError …`, exit 3.
2. YAML sanity check: `yaml_load(orig)`. Any exception becomes `InvalidConfigError` with context `File {config_file}` and leaf `str(e)`, giving exit 1 with `An error has occurred: InvalidConfigError: \n==> File .pre-commit-config.yaml\n=====> while parsing a flow node\ndid not find expected node content\n  in "<unicode string>", line 5, column 1`. **No schema validation happens.**
3. `_migrate_map` (lines 25-46): only if the YAML top level is a **list**:
   - Split into lines (keeping endings). `i` is the first line that is **not** a header line. A header line `startswith('#')` or `startswith('---')`, or is blank/whitespace-only.
   - Try `header + 'repos:\n' + rest`. If it parses as YAML, use it. This works for block sequences at column 0, because a `- ` sequence may sit at the same indent as its parent key.
   - Otherwise (`yaml.YAMLError`) use `header + 'repos:\n' + textwrap.indent(rest, '    ')`. `textwrap.indent` adds 4 spaces to every line that is not whitespace-only.
4. `_migrate_composed` (lines 58-111) works on the composed **node tree** (`yaml.compose`), with marks in **code-point indices** (verified, even with the C loader). It collects rewrites:
   - **sha → rev**: every mapping **key** scalar whose value is `sha`, at path `repos[*].sha` (a key of each repo map). The replacement keeps the quote style: `"sha"` → `"rev"`.
   - **python_venv → python**: value scalars at `repos[*].hooks[*].language` whose value is `python_venv`.
   - **stages**: item scalars at `default_stages[*]` and `repos[*].hooks[*].stages[*]` whose value is in `{commit, push, merge-commit}` are rewritten to `pre-` + value.
   - The replacement text is `{style}{new}{style}`, where `style` is `''` for plain, `'` for single-quoted, `"` for double-quoted, and **`|`/`>` for block scalars (bug, below)**.
   - The rewrites are sorted by start index in descending order and spliced as `contents[:start] + new + contents[end:]`.
   - Matching uses scalar *values* (`k.value == 'sha'`), so quoted keys match too. Paths only descend through `MappingNode`/`SequenceNode`. Aliased nodes are followed, since they are the same node objects.
   - Only the root mapping's `repos` / `default_stages` are considered. JSON-style flow input works (test `test_migrate_config_sha_to_rev_json`).
   - **Not** migrated: `system`/`script` (still valid aliases), `default_install_hook_types`, and `--hook-stage`.
5. If `contents != orig`: write with `open(config_file, 'w')` (non-atomic truncate+write, locale encoding, `\n` written as the platform newline) and print `Configuration has been migrated.` (**even when quiet**). Otherwise print `Configuration is already migrated.` unless quiet. Exit 0.

Consequences and bugs (verified):
- Line endings: on POSIX, a CRLF file that **needs** migration is rewritten with LF. If no change is needed, the file is untouched. On Windows the rewritten file gets CRLF.
- **Alias double-rewrite bug**: `default_stages: &s [commit]` plus `stages: *s` yields `default_stages: &s [pre-commitpre-commit]`, because the same node is matched twice. snp should **dedupe rewrites by node start index**.
- **Block-scalar bug**: `language: >-\n      python_venv` becomes `language: >python>` (invalid YAML). snp should treat literal/folded scalars specially, e.g. emit a plain `python` or keep the block header.
- If the fallback indentation in step 3 yields invalid YAML (for example with a leading `%YAML` directive), `yaml.compose` raises and the error escapes, giving exit 3.
- An empty file or a scalar/map root without the legacy forms prints `Configuration is already migrated.`

Verified examples are in Appendix A (MC1–MC4).

---

## 11. `sample-config` (`commands/sample_config.py`)

`pre-commit sample-config` writes exactly the following bytes to stdout (10 lines, LF endings, final newline, no trailing blank line; sha256 `8d958ac88a46c7465376a9d8b7dfc0a23565ab522249c31b25a95060ec1d887e`) and exits 0. It needs no git repo. `--color` is accepted and has no effect.
```
# See https://pre-commit.com for more information
# See https://pre-commit.com/hooks.html for more hooks
repos:
-   repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v3.2.0
    hooks:
    -   id: trailing-whitespace
    -   id: end-of-file-fixer
    -   id: check-yaml
    -   id: check-added-large-files
```
(Note `rev: v3.2.0`, which has been frozen in pre-commit for years.)

---
## 12. YAML loading semantics (PyYAML `CSafeLoader`, YAML 1.1 flavored)

### 12.1 Pipeline (`pre-commit/pre_commit/yaml.py:8-19`)
```
Loader = getattr(yaml, 'CSafeLoader', yaml.SafeLoader)       # libyaml scanner/parser + Python SafeConstructor + Python Resolver
yaml_load    = partial(yaml.load,    Loader=Loader)          # single document -> Python objects
yaml_compose = partial(yaml.compose, Loader=Loader)          # node tree (migrate-config)
Dumper = getattr(yaml, 'CSafeDumper', yaml.SafeDumper)
yaml_dump(o, **kw) = yaml.dump(o, Dumper=Dumper, default_flow_style=False, indent=4, sort_keys=False, **kw)
```
- `CSafeLoader = CParser (libyaml 0.2.5 scanner/parser/composer in C) + SafeConstructor + Resolver` (`PyYAML yaml/cyaml.py:23`). Scalar resolution and construction are **PyYAML's Python code**, not libyaml's. The syntax, its error messages and the composer messages come from libyaml/`_yaml.pyx`.
- Input is always a Python `str`, already decoded as UTF-8 with newlines normalized (§12.9). The mark name is therefore always `<unicode string>`, and marks are in **code points**.
- If PyYAML is installed without libyaml (rare, e.g. a source build), messages differ: they gain a source snippet and caret, and some wording changes (§12.8). snp should match the libyaml variant.

### 12.2 Implicit resolution of **plain** (unquoted, untagged) scalars (`PyYAML yaml/resolver.py:170-228`)
Quoted scalars (`'…'`, `"…"`) and block scalars (`|`, `>`) are **always `str`**. Plain scalars are tested against these Python regexes (`re.X`; Python `$` also matches before a final `\n`):

| Tag | Regex (PyYAML, exact) | First chars |
|---|---|---|
| bool | `^(?:yes\|Yes\|YES\|no\|No\|NO\|true\|True\|TRUE\|false\|False\|FALSE\|on\|On\|ON\|off\|Off\|OFF)$` | `yYnNtTfFoO` |
| float | `^(?:[-+]?(?:[0-9][0-9_]*)\.[0-9_]*(?:[eE][-+][0-9]+)?\|\.[0-9][0-9_]*(?:[eE][-+][0-9]+)?\|[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+\.[0-9_]*\|[-+]?\.(?:inf\|Inf\|INF)\|\.(?:nan\|NaN\|NAN))$` | `-+0123456789.` |
| int | `^(?:[-+]?0b[0-1_]+\|[-+]?0[0-7_]+\|[-+]?(?:0\|[1-9][0-9_]*)\|[-+]?0x[0-9a-fA-F_]+\|[-+]?[1-9][0-9_]*(?::[0-5]?[0-9])+)$` | `-+0123456789` |
| merge | `^(?:<<)$` | `<` |
| null | `^(?:~\|null\|Null\|NULL\|)$` (an empty plain scalar is null) | `~nN` and empty |
| timestamp | `^(?:[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]\|[0-9][0-9][0-9][0-9]-[0-9][0-9]?-[0-9][0-9]?(?:[Tt]\|[ \t]+)[0-9][0-9]?:[0-9][0-9]:[0-9][0-9](?:\.[0-9]*)?(?:[ \t]*(?:Z\|[-+][0-9][0-9]?(?::[0-9][0-9])?))?)$` | digits |
| value | `^(?:=)$` (a bare `=` **errors**: no constructor) | `=` |

The resolvers are tried in registration order per first character, and the first match wins. With the first-char filtering, the effective precedence is bool, float, int, merge, null, timestamp, value. Anything else is `str`.
**PyYAML deviates from the YAML 1.1 spec**: `y`, `Y`, `n`, `N` are **strings**, and mixed case like `yEs` or `tRue` is a string. Floats **require a dot** (`1e3` is a str) and **require a signed exponent** (`1.0e3` is a str, `1.0e+3` is a float).

Verified value table. These are the consequences for config fields: `id`, `name`, `entry`, `rev`, `language_version`, `minimum_pre_commit_version`, `args` items, `additional_dependencies` items, `files`, `exclude`, `alias` and `description` must be **strings**, while `fail_fast`, `always_run`, `verbose`, `pass_filenames` and `require_serial` must be **bools**.

| Plain scalar | PyYAML result | Example consequence |
|---|---|---|
| `yes Yes YES on On ON true True TRUE` | `True` | `fail_fast: yes` and `always_run: on` are **valid**. `args: [on]` gives `Expected string got bool`. |
| `no No NO off Off OFF false False FALSE` | `False` | `pass_filenames: no` is valid |
| `y n Y N yEs tRue` | `str` | `args: [y]` is fine |
| `~ null Null NULL` (empty) | `None` | `rev:` or `rev: ~` gives `Expected string got NoneType`; `repos:` gives `Expected array but got 'NoneType'` |
| `010`, `0700`, `0_1` | int (octal) `8`, `448`, `1` | `rev: 0700` gives `Expected string got int` |
| `0800`, `09` | **str** (not valid octal and not decimal) | `rev: 0800` is accepted |
| `0o10` | **str** | |
| `0x1F`, `0b101`, `1_000`, `+12`, `-0`, `1__0` | int | `rev: 1234567` (all-digit short SHA) gives `Expected string got int` |
| `1:30`, `190:20:30`, `-1:30` | int, sexagesimal (90, 685230, -90) | |
| `1:60` | str | |
| `1.10`, `1.0`, `.5`, `0.`, `1.e+3`, `1_0.5`, `6.8523015e+5`, `12:30:45.5` | float | `rev: 1.10`, `language_version: 3.10`, `minimum_pre_commit_version: 1.0` and `default_language_version: {python: 3.11}` all give `Expected string got float` |
| `1e3`, `1.0e3`, `1e3456`, `123e4567` | **str** (YAML 1.2 parsers would yield a float) | `rev: 1e3456` is accepted |
| `.inf +.inf -.inf .Inf .INF` | ±inf | |
| `.nan .NaN .NAN` | nan; `-.NaN` is a **str** | |
| `1.2.3`, `v1.10`, `3.2.0` | str | manifest `minimum_pre_commit_version: 3.2.0` is fine unquoted |
| `2020-01-01`, `2002-01-01 ` (trailing space) | `datetime.date` | `rev: 2020-01-01` gives `Expected string got date` |
| `2001-12-14t21:59:43.10-05:00`, `2001-12-14 21:59:43.10 -5`, `2002-01-01T00:00:00Z` | `datetime.datetime` (tz-aware) | `… got datetime` |
| `2002-1-1` | str | |
| `2020-13-45` | constructor `ValueError: month must be in 1..12` | leaf `=====> month must be in 1..12` (exit 1) |
| `0b_`, `0x_` | `ValueError: invalid literal for int() with base 2: ''` | leaf message (exit 1) |
| `=` | `ConstructorError: could not determine a constructor for the tag 'tag:yaml.org,2002:value'` | e.g. `args: [=]` is an error |

A `%YAML 1.2` directive is accepted but **does not change resolution**: `on` is still `True`.

### 12.3 Constructors and explicit tags (`PyYAML yaml/constructor.py:180-480`)
Supported tags in SafeConstructor: `!!null !!bool !!int !!float !!binary !!timestamp !!omap !!pairs !!set !!str !!seq !!map`, plus `tag:yaml.org,2002:merge`/`value` handling. Behavior details:
- int: `_` is removed, then the sign, then `0`, `0b…`, `0x…`, a leading-`0` octal, a `:` base-60 form, or decimal. Arbitrary precision.
- float: `_` is removed, the value lowercased, then the sign, `.inf`, `.nan`, base-60, or `float()`.
- bool: `bool_values[value.lower()]`, so `!!bool maybe` raises **`KeyError: 'maybe'`**, whose leaf message is `'maybe'`.
- `!!int abc` → `ValueError: invalid literal for int() with base 10: 'abc'`. `!!float abc` → `could not convert string to float: 'abc'`. `!!timestamp x` → `AttributeError: 'NoneType' object has no attribute 'groupdict'`. All of these become leaf messages via cfgv.
- `!!binary` gives `bytes` (base64, lenient). `!!set {a, b}` gives a `set`. `!!omap`/`!!pairs` give a list of 2-tuples. `!!str 1.10` gives `'1.10'`. `!!int "0x10"` gives 16. `! 123` (non-specific tag) gives int 123. `!!str` with an empty value gives `''`.
- Unknown tags (`!foo`, `!!python/tuple`, `%TAG`-expanded) raise ConstructorError `could not determine a constructor for the tag '<full tag>'`.
- Node-kind mismatch: `expected a scalar node, but found sequence` / `expected a mapping node, but found sequence` / `expected a sequence node, but found mapping`.

### 12.4 Mappings
- **Duplicate keys: silently accepted, the last value wins, and the position of the *first* occurrence is kept** (Python dict semantics): `{a: 1, b: 2, a: 3}` gives `{'a': 3, 'b': 2}`. A config with two `repos:` keys uses the second one. Many Rust YAML libraries **error** on duplicates; snp must not.
- **Merge keys `<<`** (`flatten_mapping`, `PyYAML yaml/constructor.py:180-213`):
  - the value may be a mapping or a sequence of mappings;
  - for a sequence, earlier mappings take precedence over later ones, and explicit keys in the node override every merged key;
  - merged pairs are placed **before** the node's own pairs;
  - merging is recursive (merges inside merged maps are flattened first).
  Example: `c: {<<: [{x: 1}, {x: 2, z: 3}], w: 0}` gives `{'x': 1, 'z': 3, 'w': 0}`. Error cases: `expected a mapping or list of mappings for merging, but found scalar` and `expected a mapping for merging, but found <id>`, both with the context `while constructing a mapping` and two marks.
- Keys may be any **hashable** scalar: int, float, bool, None, str, date, bytes, and tuples from omap. Python equality collapses `1`, `1.0` and `True` into one key. List or map keys give `while constructing a mapping\n  in "<unicode string>", line L, column C\nfound unhashable key\n  in "<unicode string>", line L, column C`.
- Non-string keys reach pre-commit's warning code and **crash** it: `sorted()` of mixed types gives `TypeError: '<' not supported between instances of 'str' and 'int'`, and `", ".join` over non-str gives `TypeError: sequence item 0: expected str instance, int found`. Both exit 3 (fixtures e7, e8, e9). See §16.

### 12.5 Anchors and aliases
- `&a`/`*a` aliases are resolved to the **same Python object**: shared, not copied, so there is no memory blow-up.
- **Duplicate anchor names are an error** in the C composer (`found duplicate anchor; first occurrence\n  in "<unicode string>", line 1, column 4\nsecond occurrence\n  in "<unicode string>", line 2, column 4`). This differs from YAML 1.2 tools, which allow redefinition.
- An undefined alias gives `found undefined alias\n  in "<unicode string>", line 1, column 4`. The C composer does not print the name; the pure-Python one does.
- Recursive structures (`&a [*a]`) are accepted, and harmless if under an unknown key (validation never descends there).
- Common config idiom (works): `x: &x {repo: local, hooks: []}` with `repos: [*x]` and `- <<: *x`. The anchor holder `x` produces an "Unexpected key(s) present at root: x" warning.

### 12.6 Stream-level rules
- Exactly **one document**. `---` and the end marker `...` are fine. A second document gives `expected a single document in the stream\n  in "<unicode string>", line 1, column 1\nbut found another document\n  in "<unicode string>", line 2, column 1`.
- An empty stream, comments only, or a lone `---` gives `None`, hence `Expected a Config map but got a NoneType`.
- A BOM (U+FEFF) at the start is accepted, since Python's `utf-8` codec keeps it and libyaml skips it.
- **Tabs** cannot start tokens or be used for indentation: `\ta: 1` or `-\tfoo` gives `while scanning for the next token\nfound character that cannot start any token\n  in "<unicode string>", line 1, column 1`. Tabs inside or after values are fine (`a:\t1`, `a: 1\t# c`).
- Control characters: `#x0000` etc. give `unacceptable character #x0000: control characters are not allowed\n  in "<unicode string>", position 3` (a ReaderError, which uses `position N` instead of line/column). NEL (`\x85`), LS and PS act as line breaks (`a: \x85` gives `None`).
- Comments: `a: b #c` gives `'b'`, while `a: b#c` gives `'b#c'`.
- Flow-style edge cases (libyaml): `{"a":1}` gives `{'a': 1}`, `{a:1}` gives `{'a:1': None}`, `[a:1]` gives `['a:1']`. Trailing commas `[x, ]` and `{x: 1,}` are accepted.
- Double-quoted escapes (libyaml set): `\0 \a \b \t \<TAB> \n \v \f \r \e \<SP> \" \/ \\ \N \_ \L \P \xXX \uXXXX \UXXXXXXXX`. Any other escape gives `while parsing a quoted scalar\n  in …\nfound unknown escape character\n  in …`. Single-quoted strings only support `''`.
- Multi-line plain and quoted scalars fold (`'x\n  y'` → `'x y'`; a blank line becomes `\n`).

### 12.7 YAML error message format (C loader; `PyYAML yaml/error.py:37-75`)
`MarkedYAMLError.__str__` joins these lines with `\n`:
1. `context`, if any;
2. `context_mark`, if present **and** (no problem mark, or it differs from the problem mark in name, line or column);
3. `problem`;
4. `problem_mark`;
5. `note`, if any.

A mark renders as `  in "<unicode string>", line {line+1}, column {column+1}` (two leading spaces; 1-based). The C loader adds **no snippet**. ReaderError renders as `unacceptable character #x%04x: %s\n  in "<unicode string>", position %d`.
Verified C-loader texts (libyaml problem and context strings):

| Input | Message |
|---|---|
| `repos: [\n` | `while parsing a flow node\ndid not find expected node content\n  in "<unicode string>", line 2, column 1` |
| `a: [1, 2` | `while parsing a flow sequence\n  in "<unicode string>", line 1, column 4\ndid not find expected ',' or ']'\n  in "<unicode string>", line 2, column 1` |
| `a: b: c` | `mapping values are not allowed in this context\n  in "<unicode string>", line 1, column 5` |
| `- a\nb: c` | `while parsing a block collection\n  in "<unicode string>", line 1, column 1\ndid not find expected '-' indicator\n  in "<unicode string>", line 2, column 1` |
| `a:\n  - b\n c: d` | `while parsing a block mapping\n  in "<unicode string>", line 1, column 1\ndid not find expected key\n  in "<unicode string>", line 3, column 2` |
| `a: 'x` | `while scanning a quoted scalar\n  in "<unicode string>", line 1, column 4\nfound unexpected end of stream\n  in "<unicode string>", line 1, column 6` |
| `a: "\q"` | `while parsing a quoted scalar\n  in "<unicode string>", line 1, column 4\nfound unknown escape character\n  in "<unicode string>", line 1, column 5` |
| `key: @x` | `while scanning for the next token\nfound character that cannot start any token\n  in "<unicode string>", line 1, column 6` |
| `a: *undefined` | `found undefined alias\n  in "<unicode string>", line 1, column 4` |
| `a: !foo bar` | `could not determine a constructor for the tag '!foo'\n  in "<unicode string>", line 1, column 4` |
| `? [1, 2]\n: x` | `while constructing a mapping\n  in "<unicode string>", line 1, column 1\nfound unhashable key\n  in "<unicode string>", line 1, column 3` |
| `foo: {<<: 5}` | `while constructing a mapping\n  in "<unicode string>", line 2, column 6\nexpected a mapping or list of mappings for merging, but found scalar\n  in "<unicode string>", line 2, column 11` (line 2 because the fixture had `repos: []` first) |

The pure-Python loader differs. Examples: `found character '\t' that cannot start any token`, `mapping values are not allowed here`, `expected ',' or ']', but got '<stream end>'`, `found undefined alias 'undefined'`, `special characters are not allowed`, plus snippet lines. **Target the C texts.**

### 12.8 How load errors surface
- `validate-config`: `\n==> File {path}\n=====> {str(exc)}\n`, exit 1.
- Run-type commands: `An error has occurred: InvalidConfigError: \n==> File {path}\n=====> {str(exc)}\nCheck the log at …`, exit 1.
- Any exception from `yaml.load` (YAMLError, ValueError, KeyError, AttributeError…) becomes the leaf `str(exc)`. For KeyError that is the repr of the key (`'maybe'`).

### 12.9 Encoding and newlines
- Config and manifest are read via `open(filename, encoding='utf-8')`: **strict UTF-8**, with **universal newlines** (`\r\n` and lone `\r` are converted to `\n` **before** YAML parsing, which also affects mark indexes). A UTF-8 BOM is kept and then tolerated by YAML.
- Decode failure leaf (Python `UnicodeDecodeError.__str__`): `'utf-8' codec can't decode byte 0x{b:02x} in position {pos}: {reason}`. `pos` is the **byte** offset of the offending byte and `reason` is one of:
  - `invalid start byte`: the byte is not a valid lead (0x80–0xC1, 0xF5–0xFF);
  - `invalid continuation byte`: a valid lead byte (0xC2–0xF4) followed by a bad continuation, also covering overlongs and surrogates such as `ED A0`. **The position is the lead byte.**
  - `unexpected end of data`: truncated at EOF.

  Verified: `'utf-8' codec can't decode byte 0xe9 in position 12: invalid continuation byte` and `'utf-8' codec can't decode byte 0xff in position 0: invalid start byte` (a UTF-16 file fails like this).
  Mapping from Rust `Utf8Error`: `valid_up_to()` is pos. `error_len() == None` gives "unexpected end of data". Otherwise, if `bytes[pos]` is in `0xC2..=0xF4` the reason is "invalid continuation byte", else "invalid start byte".
- `migrate-config` reads and writes with the **locale** encoding (UTF-8 on modern Linux/macOS, often cp1252 on Windows) and writes the platform newline (§10).

### 12.10 YAML emitting (used by `autoupdate` and `try-repo`; owned by those specs)
`yaml_dump` (CSafeDumper): block style, `indent=4`, insertion order, `allow_unicode=False` (non-ASCII is escaped inside double quotes, e.g. `é` becomes `"\xE9"`), line width 80 (long scalars fold). Sequences of mappings render as `-   key: v` (dash plus 3 spaces); sequences of scalars inside a mapping render as `- v` at the parent's indentation. Example:
```
repos:
-   repo: x
    rev: v1
    hooks:
    -   id: a
        args:
        - --x
        - y
        stages: []
fail_fast: true
n: null
```
Scalars are quoted when a plain form would re-resolve to a non-string under §12.2 or would be unsafe: `'1.10' 'yes' 'on' 'null' '' '0123' '2020-01-01' '0x1F' '~' '123' '1_000' '12:30' '@x' '*x' '!x' '%x' '&x' '[x]' 'a: b' 'v1.0 '`. These stay **plain**: `v1.0.0 1.2.3 abc#def -x deadbeef 1e3 y a,b it's`.
`autoupdate` rewrites only the `rev:` line, matched by `^(\s+)rev:(\s*)(['"]?)([^\s#]+)(.*)(\r?\n)$`, using `yaml_dump({'rev': new}, default_style=<original quote>)`. It falls back to re-dumping the whole file (losing comments) when the number of `rev:` lines differs from the number of non-local repos (`commands/autoupdate.py:116-159`).

---

## 13. Regular expressions (`files`, `exclude`): Python `re` semantics

- Validation: `re.compile(pattern)` must succeed. Otherwise the message is `{pattern!r} is not a valid python regex` with **no detail**, so **only the accept/reject decision must match**.
- Matching: `re.compile(p).search(path)` with no flags, against forward-slash, repo-root-relative paths (`commands/run.py:60-70`). The top-level `files`/`exclude` filter runs first, then per hook. `files: ''` matches everything. The default `exclude: '^$'` matches nothing except the empty path. `(?x)` verbose patterns in block scalars are a documented idiom.
- The accepted syntax depends on the user's Python version (pre-commit needs ≥3.10). Atomic groups `(?>…)` and possessive quantifiers `a++` exist only on ≥3.11. **Recommendation: emulate CPython 3.12/3.13.**

Acceptance matrix (verified: CPython 3.12 `re`, Rust `regex` 1.13.1, `fancy-regex` 0.19.2). Rows where the engines disagree with Python are flagged with ✗:

| Pattern | Python | regex | fancy-regex |
|---|---|---|---|
| `\.{js,ts}$` (literal braces) | OK, literal | ✗ ERR | OK |
| `a{` | OK, literal | ✗ ERR | OK |
| `a{,3}` (means {0,3}) | OK | ✗ ERR | OK, same meaning |
| `[[]` | OK (FutureWarning) | ✗ ERR | ✗ ERR |
| `(?<=a)b`, `(?<!a)b`, `a(?=b)`, `a(?!b)` | OK | ✗ ERR | OK |
| `(a)\1`, `(?P<x>a)(?P=x)` | OK | ✗ ERR | OK |
| `(?<x>a)` | **ERR** on 3.12 | ✗ OK | ✗ OK |
| `\Z` | OK (absolute end) | ✗ ERR | OK but **means end-or-before-final-newline** ✗ |
| `\z` | ERR | ✗ OK | ✗ OK |
| `(?>a)`, `a++` | OK (3.11+) | ✗ ERR / OK | OK |
| `(a)?(?(1)b\|c)` | OK | ✗ ERR | OK |
| `(?(1)a\|b)` with no group 1 | ERR | ERR | ✗ OK |
| `\p{L}`, `\e`, `\h`, `\K`, `\N{DASH}`, `(*FAIL)` | ERR | mixed | ✗ OK |
| `\0` (NUL) | OK | ✗ ERR | ✗ ERR |
| `(?#comment)a` | OK | ✗ ERR | OK |
| `(?a)\w` (ASCII flag) | OK | ✗ ERR | ✗ ERR |
| `(?L)a` | ERR | ERR | ERR |
| `[\w-.]` | ERR | ERR | ERR |
| `a**` | ERR ("multiple repeat") | ✗ OK | ERR |
| `(?i:a)b`, `(?-i:a)`, `(?i-s:a)`, `(?s).`, `(?m)^a$`, `\d+`, `\/`, `\:`, `\-`, `\@`, `[\\/]`, `[]a]`, `[^]a]` | OK | OK | OK |

Matching-semantics differences that matter rarely (paths almost never end with `\n`):
- Python `$` (without `re.M`) matches at the end **or before a final `\n`**; Rust `$` matches only at the end. `a$` on `"a\n"` is True in Python and False in Rust.
- Python `\Z` matches only at the very end; fancy-regex `\Z` also matches before a final newline.
- Unicode classes (`\d`, `\w`, `\s`) are Unicode-aware in both by default. Case-folding details can differ (`(?i)straße` vs `STRASSE` is False in all of them).

**Recommendation for snp:**
- (a) Write a small Python-`re` syntax front end, ported from CPython's `_parser.py` grammar, used **only** for accept/reject parity and to produce a normalized AST.
- (b) Lower that AST to `regex` syntax when no backtracking features are used (fast path: linear time, SIMD prefilters). Otherwise lower to `fancy-regex`.
- (c) Translate `\Z` to `\z`; Python `$` to `(?:\n?\z)`, but only outside character classes and when `(?m)` is not in effect (consuming the newline is safe because only `is_match` is used); a literal `{` to `\{`; `\0`/octal escapes to `\x{..}`; and `(?a)` to ASCII classes.
- (d) Compile once per distinct pattern string and cache across hooks.

---

## 14. Platform differences

| Area | Windows | macOS / Linux |
|---|---|---|
| Filenames given to `files`/`exclude` | `\` is converted to `/` before matching (`commands/run.py:123-124`), which explains warnings W5 and W7. The same normalization applies to `git ls-files` output in the meta hooks. | git already gives `/` |
| Meta-hook entry | `shlex.quote(sys.executable)` with POSIX quoting, then shlex-split by the runner (`clientlib.py:350-355`). snp should run meta hooks in-process. | same |
| Color auto-detection | VT mode must be enabled on **stderr** (`SetConsoleMode`). If that fails, there is no color. | `isatty(stderr)` and `TERM != dumb` |
| `migrate-config` I/O | locale encoding (often cp1252) and CRLF output | UTF-8 and LF output (CRLF input is normalized to LF when rewritten) |
| Config read | UTF-8 with universal newlines (same everywhere) | same |
| Case-insensitive filesystems (macOS default, Windows) | Opening `-c` paths is case-insensitive. The meta-hook `files` regex `^\.pre\-commit\-config\.yaml$` is case-**sensitive** against git paths. | |

---

## 15. Version differences: installed 4.1.0 vs reference 4.6.2 (diffed and run)

| Behavior | 4.1.0 | 4.6.2 |
|---|---|---|
| Language names | includes `script`, `system`; no `unsupported*` | `unsupported`, `unsupported_script`; `system`/`script` are aliases (translated) |
| Meta hook `language` | must be `system` (`Expected one of system but got: 'unsupported'`) | must be `unsupported` (`language: system` is **rejected**) |
| `default_language_version` keys | allowed `script`, `system` | allowed `unsupported`, `unsupported_script` |
| Invalid `stages` error context | missing `At key: stages` (straight to `At index N`) | `==> At key: stages` then `==> At index N` (fixed in 4.4.0) |
| Manifest that is a dict | generic `Expected array but got 'dict'` | forward-compat "version 5 required" (§4.2) |
| Everything else in this document (cfgv 3.4.0 vs 3.5.0 only added the unused `KeyValueMap`) | same | same |

**Compat recommendation:** emulate 4.6.2, and additionally accept `language: system` on meta hooks, normalizing it to `unsupported`. That keeps configs written for ≤4.3 working. See Open questions.

---

## 16. Where pre-commit crashes (exit 3) and what snp should do instead

pre-commit raises non-ValidationError exceptions during validation in these cases (all verified):

| Input | pre-commit output (exit 3) | Recommended snp behavior |
|---|---|---|
| `minimum_pre_commit_version: "abc"` / `""` / `"4.6.3rc1"` | `An unexpected error has occurred: ValueError: invalid literal for int() with base 10: 'abc'` | A ValidationError under `At key: minimum_pre_commit_version` with leaf `invalid literal for int() with base 10: 'abc'` (same text, exit 1). Alternatively keep exit 3 for strictness; see Open questions. |
| A non-string root key (`1: x`, `true: x`, `null: x`) | `TypeError: sequence item 0: expected str instance, int found` | Warn using Python `str()` of the keys, sorted by `(type rank, str)`, e.g. `Unexpected key(s) present at root: 1` |
| Mixed root keys (`1: x` and `foo: y`) | `TypeError: '<' not supported between instances of 'str' and 'int'` | same as above |
| Unhashable stage, language or type entry (`stages: [[a]]`, `language: [x]`, `types: [[a]]`) | `TypeError: unhashable type: 'list'` | a `one_of` style error: `Expected one of … but got: ['a']` / `Type tag ['a'] is not recognized.  Try upgrading identify and pre-commit?` |
| `migrate-config` with a missing file | `FileNotFoundError …` | keep (exit 3) or give a clean error |
| Fallback list migration producing invalid YAML | `ScannerError …` | report as InvalidConfigError (exit 1) |

In all these cases snp must **never panic**. Every path must produce a `Result`.

---

## 17. Ideas from lefthook relevant to the config layer (for snp extensions)

lefthook (`refs/lefthook/internal/config/*.go`) features worth considering, **namespaced so that pre-commit compatibility is kept**:
- **Local override file** merged on top of the main file (`lefthook-local.yml`, `.lefthook-local.yml`, `.config/lefthook-local.yml`; `loader.go:36-37`), git-ignored for per-developer tweaks such as skipping a slow hook. snp equivalent: an optional `.snp-local.yaml` (or `.pre-commit-config.local.yaml`) deep-merged by repo URL and hook id.
- **`extends`** (local files and globs, recursion guarded: `possible recursion in extends: path %s is specified multiple times`, `loader.go:333-370`) and **`remotes`** (config fetched from git at a ref). Useful for org-wide shared configs.
- **Config-path env override** `LEFTHOOK_CONFIG` (`loader.go:131`). Possible snp equivalent: `SNP_CONFIG`.
- **`min_version`** for the tool itself, alongside pre-commit's `minimum_pre_commit_version`. snp could add `minimum_snp_version`.
- Multiple formats (`.yml .yaml .json .jsonc .toml`). JSON is already valid YAML. A TOML config is a pure extension.
- Per-job options pre-commit lacks: `env` (a map), `root` (subdir cwd), `priority`/`parallel` (concurrency), `glob` (globs instead of regex), `skip`/`only` (conditions such as `merge`, `rebase`, `ref: main`, and run commands), `tags`/`exclude_tags`, `stage_fixed` (re-stage fixed files), `fail_text`, `timeout`, `interactive`, `use_stdin`.

Compatibility constraints for any extension:
- pre-commit **warns, but does not fail**, on unknown root and repo keys (W1, W2).
- For hook keys it warns only at hook build time (W11).
- Unknown keys inside `ci:` are silent. pre-commit.ci owns that namespace, so snp must **not** put its keys there.
- Extensions inside `.pre-commit-config.yaml` therefore keep the file usable by upstream pre-commit, at the cost of warnings. A separate snp file avoids even those.

---
## 18. Implementation notes for snp

### 18.1 Module/crate layout (suggested)
- `snp-yaml`: a PyYAML-`CSafeLoader`-compatible loader: events, then a composer with an arena node tree, then a constructor into `PyValue`, plus a 1.1-aware emitter for autoupdate and try-repo.
- `snp-pyre`: a Python-`re`-compatible pattern front end that lowers to `regex` or `fancy-regex` (§13).
- `snp-config`: the cfgv-style engine (or the user's `cfgv` crate, once patched; §18.4), the pre-commit schemas, the custom validators, defaults, the typed model, the hook merging of §6, migrate-config, and sample-config.
- Workspace-wide `#![forbid(unsafe_code)]`, `clippy::pedantic` plus `unwrap_used`/`expect_used`/`panic`/`indexing_slicing` denied in library code. Every user-input path returns `Result`.

### 18.2 YAML layer (byte-compatibility critical)
- **Parser:** `libyaml-safer = "=0.3.0"`, a line-by-line safe port of libyaml (`#![deny(unsafe_code)]`, with one audited `#[allow(unsafe_code)]` in `reader.rs` for partial UTF-8 decoding; vendor and patch it if `forbid` must be absolute).
  It was verified here to produce the **same problem/context strings and the same 0-based line/column (in characters)** as PyYAML's C loader: `did not find expected ',' or ']'` / `while parsing a flow sequence`, `found character that cannot start any token` / `while scanning for the next token`, `found unknown escape character` / `while parsing a quoted scalar`, and reader errors `control characters are not allowed` with a byte `offset` equal to PyYAML's `position`.
  Its `Mark.index` is a **byte** offset, which is convenient for Rust slicing in migrate-config. PyYAML's composer marks are code-point indexes, but that only matters inside Python.
  Do **not** use serde-based YAML 1.2 deserializers (`serde_yaml`/`yaml_serde`, `serde-saphyr`, `saphyr`, `yaml-rust2`) for config semantics. They resolve `yes/on/off` as strings, `1e3` as a float, and `0o17` as an int; they have no timestamps; and some reject duplicate keys. prek uses serde-saphyr and inherits these differences.
- **Pre-processing:**
  1. Read bytes, rejecting files over a sane cap (e.g. 8 MiB) with a clean error.
  2. Strict UTF-8 decode with the exact Python error text (§12.9).
  3. Universal-newline normalization (`\r\n` and `\r` to `\n`). Allocate only if a `\r` is present (`Cow<str>`).
  4. Keep a leading BOM; libyaml skips it.
- **Composer** (own code, mirroring `_yaml.pyx`):
  - single document, with `expected a single document in the stream` + `but found another document`;
  - anchors in a `HashMap<&str, NodeId>`, with **duplicate anchor = error** and **undefined alias = error** (exact texts in §12.5/§12.7);
  - nodes stored in an arena `Vec<Node>` addressed by `NodeId(u32)`, so aliases are shared **ids**, never deep copies. This gives no exponential "billion laughs" blow-up and allows recursion.
  - Keep `style`, `tag`, `start`/`end` byte offsets and line/column per node for migrate-config and optional rich diagnostics.
  - Cap nesting depth (e.g. 512) with a clean ValidationError.
- **Resolver/constructor** (own code): implement the PyYAML regexes of §12.2 exactly. Hand-written matchers are faster than `regex` here; fuzz them against PyYAML. Implement the constructors of §12.3, including merge-key flattening order (§12.4) and Python-dict duplicate semantics: an `IndexMap` with insertion order, where replacing a value keeps the original position.
- **Value model:**
  ```rust
  enum PyValue {
      None,
      Bool(bool),
      Int(PyInt),          // i64 fast path + big decimal string
      Float(f64),
      Str(Box<str>),
      Bytes(Box<[u8]>),
      Date(..),
      DateTime(..),        // keep the source text for repr
      List(Vec<PyValue>),
      Tuple(Vec<PyValue>),
      Set(..),
      Dict(IndexMap<PyKey, PyValue>),
  }
  ```
  `type_name()` returns exactly `NoneType bool int float str bytes date datetime list tuple set dict`. `repr()` follows §2.6 exactly, including Python float repr (shortest round-trip, e.g. via the `ryu` crate, adapted to Python's exponent thresholds and `1.0`/`1e+16`/`1e-05` formatting). Key equality must follow Python: `1 == 1.0 == True`, and NaN keys never compare equal to each other. Mapping keys need a total order for "Unexpected key(s)" (§16).
  For recursive aliases under unknown keys, either keep `NodeId` references lazily (preferred: validate straight from the arena, constructing only the known keys) or reject recursion with a clean error.
- **Performance:** a typical config is under 10 KiB, so a single pass is cheap. Avoid building a full owned tree. Validate directly against the arena, materializing only the typed `Config` structs (`Box<str>`, `SmallVec<[_; 4]>` for short lists, `Arc<str>` for repo URLs shared across hooks, a `u16` bitset for the 11 stages, interned tag ids from `file-identify` for types). Load the config **once** per process; pre-commit re-loads it in several places, but re-validation is only needed for message parity in meta hooks (§9.4).

### 18.3 Validation engine and messages
- Reproduce cfgv's **item order per schema** (tables in §3 and §4) and the "first error aborts; earlier warnings stay" behavior. The order decides which single error the user sees.
- Model errors as `struct ValidationError { contexts: Vec<Cow<'static, str>>, leaf: String }`, where contexts are pushed while unwinding (cheap; strings are built only on the error path). Render exactly as in §2.2: a leading `\n`, `==> ` lines, `=====> ` leaf, no trailing newline.
- Warnings go through a sink trait (`fn warn(&self, msg: String)`), so that `run` prints `[WARNING] …` with color rules (§8.2) and in-process meta hooks capture bare lines (§9.4). Emit them in validation order, interleaved correctly with errors (chronological; §8.2).
- Version check: compare against a single constant, `PRE_COMMIT_COMPAT_VERSION` (e.g. `"4.6.2"`). Parse components with Python-`int()`-compatible rules (§5.9): whitespace, sign, underscores, Unicode `Nd` digits. On failure produce a ValidationError (recommended; §16).
- Keep all message templates in one `messages.rs` with golden tests copied from Appendix A.

### 18.4 Using the user's crates
- **`cfgv` 0.2.0 (cfgv-rs)** has gaps relative to Python cfgv. These must be fixed upstream before it can drive snp byte-for-byte:
  1. `Value::repr` for strings is `'{s}'` with no Python quoting or escaping, so `"it's"` and `'^a\\.txt$'` come out wrong. Floats use Rust `{}`, so `1.0` prints as `1`.
  2. `ConditionValue::describe_opposite` formats tuple members with `Display`, giving `(local, meta)` where Python gives `('local', 'meta')`, and lacks the single-element `('x',)` form.
  3. There are no `date`/`datetime`/`set`/`tuple` value types and no big ints.
  4. `load_from_filename` uses `read_to_string`, so the error text differs from Python's (§12.9) and there is no universal-newline handling.
  5. `Validator` has no custom-item variant for StagesMigration, LanguageMigration, WarnMutableRev, OptionalSensibleRegex, NotAllowed and the deprecated-stage warnings. A `Validator::Custom(Arc<dyn Fn(&Map, &mut dyn WarnSink) -> Result<…>>)` plus `apply_default` hook would cover them.
  6. `CheckFn::Regex` uses Rust `regex` syntax, which conflicts with Python `re` (§13). It must be pluggable.
  7. There is no warning sink: WarnAdditionalKeys uses a callback but there is no context-aware logger.
  8. `check_one_of` sorts `Display` strings. That is fine for string choices but should mirror Python `sorted()` on the values.

  The alternative is a purpose-built, schema-specific validator in `snp-config` (simpler and faster), with cfgv kept for generic use.
- **`file-identify` 0.4.0**: 31 tags missing compared with identify 2.6.20 (§1.5). Sync it to identify 2.6.20 (the latest) and add a CI job that diffs `ALL_TAGS` against the PyPI `identify` package.

### 18.5 Merging and the typed model
- `Hook` struct fields as in §6.6. Build it with `manifest.clone()` overlaid by each config key present (whole-key replace), then resolve `language_version` and `stages` and apply the env-less guard (§6.3, §6.4). Emit W11 for unknown keys **after** merging, sorted.
- Keep the **original order** of repos and hooks. Do not dedupe ids. `run <id>` matches `id` or `alias`.
- Precompile `files`/`exclude` once per distinct string (an `FxHashMap<Box<str>, Arc<CompiledPattern>>`), since the top-level `exclude` and the many default `''`/`'^$'` patterns repeat.
  - Fast paths: `''` matches everything and `'^$'` matches only the empty string. Skip regex evaluation for both.
- Language aliases: store an enum `Language` with `from_config(&str) -> Option<Language>` that accepts `system` and `script`. Messages always print the canonical name.

### 18.6 Meta hooks
Implement the three meta hooks in-process (no Python). Choose a non-overridable internal `entry`, e.g. a sentinel string that never reaches a shell, for display parity in `--verbose`/debug output. Reproduce:
- the default `name`/`files`/`verbose` values;
- the config re-load with bare warnings in the output;
- the config-only (unmerged) view used by `check-useless-excludes`;
- the `always_run`/`fail` skip in `check-hooks-apply`;
- `{exclude!r}` printed as a Python repr;
- argv semantics (args plus filenames are all config paths; `.pre-commit-config.yaml` if empty).

Respect `-c`? pre-commit doesn't (quirk). Matching the quirk is the safest choice for compatibility; see Open questions.

### 18.7 migrate-config
Port the algorithm of §10 using the composed node tree's **byte** offsets. Improvements that cannot break valid outputs:
- dedupe rewrites by `(start, end)`;
- for literal/folded block scalars, rewrite only the scalar content span (or emit a plain token);
- write **atomically** (temp file in the same directory, `fsync`, rename, keep permissions);
- preserve the original line endings: detect CRLF and write CRLF back, instead of Python's platform-dependent behavior. Decide via Open questions.

Messages and exit codes stay as in §10.

### 18.8 Testing strategy
- **Golden tests:** every Appendix A fixture → exact stdout bytes and exit code. Generate more by running pre-commit 4.6.2 in CI (pinned venv) over a corpus: all fixtures here, plus the `.pre-commit-config.yaml` files of the top ~500 GitHub repos.
- **Differential fuzzing:** `cargo-fuzz` generates YAML. A Python oracle (`yaml.load(..., CSafeLoader)` → JSON-like canonical repr, run with `python -I`) is compared with snp's `PyValue` repr, covering resolution, merge keys, duplicates and errors (message text included).
- **Regex parity fuzzing:** random patterns through `re.compile` versus `snp-pyre` acceptance, plus `re.search` versus snp matching on random paths.
- **Property tests:** `render(error)` always starts with `\n` and contains no trailing newline. Validation is deterministic. No panics (fuzz with `-C panic=abort` and `debug-assertions`).

### 18.9 Pitfalls checklist
- `Expected array but got 'dict'` quotes the type, while `Expected string got dict` does not, and `Expected a Config map but got a dict` reads "a dict" (and "a int").
- `At key:` contexts are **missing** for Required-key-missing errors and for `'entry' cannot be overridden`.
- No `At index` context for arrays of maps (repos, hooks, manifest entries).
- `default_stages` default includes `manual`. `stages: []` means "inherit default_stages".
- The `language` override on remote hooks is allowed and changes the `default_language_version` lookup.
- validate-config with no args validates nothing.
- Warnings print on **stdout**. Color auto-detection looks at **stderr**.
- PyYAML quirks: `y`/`n` are strings; `1e3` is a string; `0800` is a string; `0700` is an int; duplicate keys are allowed; duplicate anchors are an error; `%YAML 1.2` is ignored.
- Python `repr` doubles backslashes in regex warnings (`'^a\\.txt$'`), yet W5/W7 print `fwd_slash_re` **raw** (`[\\/]` stays as written).
- The manifest `stages` deprecation warning appears only at first clone (W10). Config-level warnings (W8, W9) appear on every load.

---

## 19. Open questions

1. **Which version string should `minimum_pre_commit_version` compare against and print?** Options: a fixed compat version (`4.6.2`), or snp's own version. The message text also mentions `pip install --upgrade pre-commit`. Should snp keep it verbatim (drop-in) or say "upgrade snp"? Proposal: compare against `PRE_COMMIT_COMPAT_VERSION`, keep the text verbatim, and add an optional `minimum_snp_version` key.
2. **Branding in messages:** W8/W9/W10 and others say "`pre-commit migrate-config`" / "`pre-commit autoupdate`". Keep them verbatim, use the invoked `argv[0]` basename, or always say `snp`? This matters if snp is installed under the name `pre-commit`.
3. **Crash parity (§16):** reproduce pre-commit's exit-3 crashes for odd inputs, or turn them into clean validation errors (exit 1)? Recommended: clean errors. No tool depends on exit 3.
4. **Output ordering when piped (§8.2):** reproduce Python's block-buffer reordering (validation errors printed after later warnings), or stay chronological? Recommended: chronological.
5. **Meta hook `language: system`:** accept it as an alias (4.1.0 compat) or reject it like 4.6.2?
6. **`.pre-commit-config.yml` fallback:** prek supports it, pre-commit does not. A fallback only when `.yaml` is absent seems safe. What should happen when both exist?
7. **Regex dialect target:** which CPython version's `re` to emulate (3.12 rejects `(?<name>…)`; 3.11 added `(?>…)` and `a++`)? Proposal: 3.12.
8. **Meta hooks and `-c`:** keep the quirk (always `.pre-commit-config.yaml`) or honor `-c`?
9. **migrate-config line endings and encoding:** keep Python's behavior (LF on POSIX, CRLF on Windows when rewriting, locale encoding) or preserve the original endings and use UTF-8? Proposal: UTF-8 everywhere and preserve the original endings.
10. **Unknown-key warnings for hooks at validate time:** pre-commit only warns at hook build. snp could warn earlier (in `validate-config`), but that changes output. Proposal: keep parity by default and expose a `--strict` flag.
11. **YAML error snippet:** the pure-Python PyYAML adds source snippets. Optionally show richer diagnostics (with caret) behind an env/flag (e.g. `SNP_RICH_ERRORS=1`), keeping the default byte-identical?
12. **Big integers and exotic YAML types** (`!!set`, `!!omap`, timestamps with timezones): exact repr parity for these in error contexts is low value. Is approximate repr acceptable?
13. **identify drift:** pin to identify 2.6.20's tag set, or track upstream continuously? The error text tells users to "upgrade identify and pre-commit", which is meaningless for snp.

---

## Appendix A. Verified fixtures (4.6.2; `validate-config` unless noted)

Output is stdout. "→" shows the exit code. A leading blank line is part of every validation-error block.

**V1** `repos: []` → no output, 0. **V2** no filenames → no output, 0. **V3** `nonexist.yaml`:
```

=====> nonexist.yaml is not a file
```
→ 1 (a directory gives the same message).

**V4** empty file or comment-only file:
```

==> File empty.yaml
=====> Expected a Config map but got a NoneType
```
→ 1. A scalar `foo` gives `got a str`, and a top-level list gives `got a list`.

**V5** local hook missing `name`: see §2.2 → 1.

**V6** `language: bogus` on a local hook:
```

==> File a1.yaml
==> At Config()
==> At key: repos
==> At Repository(repo='local')
==> At key: hooks
==> At Hook(id='a')
==> At key: language
=====> Expected one of conda, coursier, dart, docker, docker_image, dotnet, fail, golang, haskell, julia, lua, node, perl, pygrep, python, r, ruby, rust, swift, unsupported, unsupported_script but got: 'bogus'
```

**V7** `stages: [commit, push, bogus]` (local):
```
...
==> At Hook(id='a')
==> At key: stages
==> At index 2
=====> Expected one of commit-msg, manual, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg but got: 'bogus'
```
No deprecation warning is printed, because the hook failed before its warning items ran.

**V8** `files: '['` gives `==> At key: files` / `=====> '[' is not a valid python regex`.

**V9** `types: [python, nonsense]` gives `==> At key: types` / `==> At index 1` / `=====> Type tag 'nonsense' is not recognized.  Try upgrading identify and pre-commit?`

**V10** (warnings only, exit 0). Input:
```yaml
repos:
- repo: https://github.com/pre-commit/pre-commit-hooks
  rev: main
  hooks:
  - id: trailing-whitespace
    stages: [commit]
    files: 'foo/*'
    exclude: 'a[\\/]b'
    language: system
    foo: bar
  extra: 1
default_stages: [commit, push]
files: '/*'
exclude: '[/\\]'
bogus_top: 1
ci: {skip: [x]}
```
Output (in this exact order):
```
[WARNING] The 'files' field in hook 'trailing-whitespace' is a regex, not a glob -- matching '/*' probably isn't what you want here
[WARNING] pre-commit normalizes slashes in the 'exclude' field in hook 'trailing-whitespace' to forward slashes, so you can use / instead of [\\/]
[WARNING] hook id `trailing-whitespace` uses deprecated stage names (commit) which will be removed in a future version.  run: `pre-commit migrate-config` to automatically fix this.
[WARNING] The 'rev' field of repo 'https://github.com/pre-commit/pre-commit-hooks' appears to be a mutable reference (moving tag / branch).  Mutable references are never updated after first install and are not supported.  See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.  Hint: `pre-commit autoupdate` often fixes this.
[WARNING] Unexpected key(s) present on https://github.com/pre-commit/pre-commit-hooks: extra
[WARNING] top-level `default_stages` uses deprecated stage names (commit, push) which will be removed in a future version.  run: `pre-commit migrate-config` to automatically fix this.
[WARNING] Unexpected key(s) present at root: bogus_top
[WARNING] The top-level 'files' field is a regex, not a glob -- matching '/*' probably isn't what you want here
[WARNING] pre-commit normalizes the slashes in the top-level 'exclude' field to forward slashes, so you can use / instead of [/\\]
```
The hook key `foo` is **not** reported here (W11 is a run-time warning).

**V11** repo-level fixtures (prefix `==> File …`, `==> At Config()`, `==> At key: repos` omitted):

| Input (repo entry) | Last lines |
|---|---|
| `repo: https://x/y`, `hooks: []` | `==> At Repository(repo='https://x/y')` / `=====> Missing required key: rev` |
| `repo: local`, `rev: v1`, `hooks: []` | `==> At Repository(repo='local')` / `=====> Expected rev to be absent when repo is any of ('local', 'meta'), found rev: 'v1'` |
| `repo: local`, `rev: 1.0` | `… found rev: 1.0` |
| `repo: https://x/y`, `rev: v1` | `=====> Missing required key: hooks` |
| `rev: v1`, `hooks: []` (no repo) | `==> At Repository(repo=MISSING)` / `=====> Missing required key: repo` |
| `repos: {}` | `==> At key: repos` / `=====> Expected array but got 'dict'` |
| `repos: [foo]` | `=====> Expected a Repository map but got a str` |
| `repo: 123` | `==> At Repository(repo=123)` / `==> At key: repo` / `=====> Expected string got int` |
| `rev: 1.10` / `2020-01-01` / `0123` / `~` / `1234567` / `0700` | `==> At key: rev` / `=====> Expected string got float` / `got date` / `got int` / `got NoneType` / `got int` / `got int` |
| `rev: ""` | warning W3 only, exit 0 |
| `rev: 1e3456` / `0800` | valid (strings) |
| `hooks: [{id: 1}]` | `==> At Hook(id=1)` / `==> At key: id` / `=====> Expected string got int` |
| `hooks: [foo]` | `=====> Expected a Hook map but got a str` |
| `hooks: {}` | `=====> Expected array but got 'dict'` |
| `hooks: [{name: x}]` | `==> At Hook(id=MISSING)` / `=====> Missing required key: id` |

**V12** top-level fixtures:

| Input | Last lines / result |
|---|---|
| `minimum_pre_commit_version: "99"` | `==> At key: minimum_pre_commit_version` / ``=====> pre-commit version 99 is required but version 4.6.2 is installed.  Perhaps run `pip install --upgrade pre-commit`.`` |
| same with `repos: 5` | still the version error (it is checked first) |
| `minimum_pre_commit_version: 1.0` | `=====> Expected string got float` |
| `"abc"` / `""` / `"4.6.3rc1"` | `An unexpected error has occurred: ValueError: invalid literal for int() with base 10: 'abc'` + `Check the log at …`, exit 3 |
| `"4.6.2"` | valid. `"4.6.2.0"` gives the version error |
| `default_install_hook_types: [pre-commit, commit]` | `==> At key: default_install_hook_types` / `==> At index 1` / `=====> Expected one of commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg but got: 'commit'` |
| `default_install_hook_types: pre-push` | `=====> Expected array but got 'str'` |
| `default_language_version: {python: python3.11, rubyx: "2"}` | `==> At key: default_language_version` / `==> At DefaultLanguageVersion()` / `=====> Additional keys found: rubyx.  Only these keys are allowed: conda, …, unsupported_script` |
| `default_language_version: {python: 3.11}` | `… ==> At DefaultLanguageVersion()` / `==> At key: python` / `=====> Expected string got float` |
| `default_language_version: [python]` | `=====> Expected a DefaultLanguageVersion map but got a list` |
| `default_stages: [commit, manual, bogus]` | `==> At key: default_stages` / `==> At index 2` / `=====> Expected one of commit-msg, manual, … but got: 'bogus'` |
| `default_stages: commit` | `=====> Expected array but got 'str'` |
| `fail_fast: yes` | valid. `"true"` gives `Expected bool got str`, and `1` gives `Expected bool got int` |
| `exclude: "(?P<x"` | `=====> '(?P<x' is not a valid python regex` |
| `ci: [1]` / `ci: ~` | `==> At key: ci` / `=====> Expected dict got list` / `got NoneType` |
| `default_stages: []` | valid |

**V13** meta fixtures:

| Input | Result |
|---|---|
| `id: bogus` | `==> At Hook(id='bogus')` / `==> At key: id` / `=====> Expected one of check-hooks-apply, check-useless-excludes, identity but got: 'bogus'` |
| `id: identity`, `entry: echo` | `==> At Hook(id='identity')` / `=====> 'entry' cannot be overridden` |
| `id: identity`, `language: system` | `==> At key: language` / `=====> Expected one of unsupported but got: 'system'` |
| `check-hooks-apply` with `files: "x/*"`, `stages: [commit]`, `foo: 1` | W4 + W8 warnings, exit 0 |

**V14** manifests (`validate-manifest`):

| Input | Result |
|---|---|
| `- {id: a, name: a, entry: a, language: system}` | valid, 0 |
| `hooks:` map | the v5 block of §4.2, exit 1 |
| missing language | `==> File h3.yaml` / `==> At Hook(id='a')` / `=====> Missing required key: language` |
| `minimum_pre_commit_version: "99.0"` | `==> At Hook(id='a')` / `==> At key: minimum_pre_commit_version` / ``=====> pre-commit version 99.0 is required but version 4.6.2 is installed.  Perhaps run `pip install --upgrade pre-commit`.`` |
| `[]` | valid |
| empty | `==> File h6.yaml` / `=====> Expected array but got 'NoneType'` |
| unknown keys, legacy stages, `files: "a/*"` | valid, **no warnings** |
| duplicate ids | valid |

**V15** YAML-level fixtures (validate-config):

| Input | Leaf |
|---|---|
| `repos: [\n` | `while parsing a flow node\ndid not find expected node content\n  in "<unicode string>", line 2, column 1` |
| `repos: []\n\tfoo: 1\n` | `while scanning for the next token\nfound character that cannot start any token\n  in "<unicode string>", line 2, column 1` |
| `repos: []\n---\nrepos: []\n` | `expected a single document in the stream\n  in "<unicode string>", line 1, column 1\nbut found another document\n  in "<unicode string>", line 2, column 1` |
| `…args: [=]` | `could not determine a constructor for the tag 'tag:yaml.org,2002:value'\n  in "<unicode string>", line 8, column 12` |
| `foo: 2020-13-45` | `month must be in 1..12` |
| bytes `\xff\xfe…` | `'utf-8' codec can't decode byte 0xff in position 0: invalid start byte` |
| `# \xe9` at byte 12 | `'utf-8' codec can't decode byte 0xe9 in position 12: invalid continuation byte` |
| UTF-8 BOM + `repos: []` | valid |
| `repos: []\nrepos: [1]` | duplicate key: the last wins, giving `==> At key: repos` / `=====> Expected a Repository map but got a int` |
| `repos: !!binary aGVsbG8=` | `=====> Expected array but got 'bytes'` |
| `files: !!set {a}` | `=====> Expected string got set` |
| anchors + merge (`x: &x {repo: local, hooks: []}`, `repos: [*x, {<<: *x, hooks: […, <<: {files: "^b"}]}]`) | valid, warning `Unexpected key(s) present at root: x` |
| `%YAML 1.2\n---\nrepos: []\nfail_fast: on` | valid (`on` is still True) |
| `repos: []\n1: x` | `An unexpected error has occurred: TypeError: sequence item 0: expected str instance, int found`, exit 3 |
| `repos: []\n1: x\nfoo: y` | `… TypeError: '<' not supported between instances of 'str' and 'int'`, exit 3 |
| `stages: [[a]]` / `language: [x]` / `types: [[a]]` | `… TypeError: unhashable type: 'list'`, exit 3 |

Non-string repr fixtures: `id: 2020-01-01` gives `At Hook(id=datetime.date(2020, 1, 1))`; `id: [1, "x"]` gives `At Hook(id=[1, 'x'])`; `id: {a: 1}` gives `At Hook(id={'a': 1})`; `id: !!binary aGk=` gives `At Hook(id=b'hi')`; `id: .nan` gives `At Hook(id=nan)`; `id: "it's"` gives `At Hook(id="it's")`; `language: yes` gives `but got: True`; `language: 1.5` gives `but got: 1.5`.

**R (run-time, `pre-commit run --all-files` in a git repo):**
- R1 Local hook with keys `foo`, `zzz`: the first output line is `[WARNING] Unexpected key(s) present on local => echo1: foo, zzz`.
- R2 Invalid config at run: `An error has occurred: InvalidConfigError: ` (trailing space), then the rendered block, then `Check the log at {PRE_COMMIT_HOME}/pre-commit.log`, exit 1. A missing config gives `An error has occurred: InvalidConfigError: \n=====> .pre-commit-config.yaml is not a file\nCheck the log at …`.
- R3 `language: system` + `language_version: python3` prints the following, exit 1. The `additional_dependencies` variant is analogous.
  ```
  [ERROR] The hook `lv` specifies `language_version` but is using language `unsupported` which does not install an environment.  Perhaps you meant to use a specific language?
  ```
- R4 `default_language_version: {unsupported: x}` + a system hook: the same `language_version` error, exit 1.
- R5 `default_stages: []` + a hook without stages: no output, exit 0. `default_stages: [pre-push]` + a hook with `stages: []`, then `run ad`: prints ``No hook with id `ad` in stage `pre-commit` ``, exit 1.
- R6 First clone of a repo whose manifest uses `stages: [commit, push, manual, commit]` and `[merge-commit]`, followed by a second repo entry naming a missing id. Output in order, exit 1:
  ```
  [INFO] Initializing environment for {repo}.
  [WARNING] repo `{repo}` uses deprecated stage names (commit, push, merge-commit) which will be removed in a future version.  Hint: often `pre-commit autoupdate --repo {repo}` will fix this.  if it does not -- consider reporting an issue to that repo.
  [WARNING] Unexpected key(s) present on {repo} => hello: bar, foo
  [ERROR] `nonexistent` is not present in repository {repo}.  Typo? Perhaps it is introduced in a newer version?  Often `pre-commit autoupdate` fixes this.
  ```
  In the W11 line, `bar` comes from the config and `foo` from the manifest.
- R7 A config `stages: []` overriding manifest `stages: [merge-commit]` makes the hook run in the `pre-commit` stage (default_stages = all). A config `args: [--x]` replaces the manifest args, so the `entry: echo` hook prints `--x .pre-commit-config.yaml a.txt`.

**M (meta hooks during `run --all-files`):**
- M1 `check-hooks-apply` failing: `Check hooks apply to the repository......................................Failed` / `- hook id: check-hooks-apply` / `- exit code: 1` / blank / `nofiles does not apply to this repository` / blank.
- M2 `check-useless-excludes` failing: see §9.2, with output lines between blank lines.
- M3 Bare warnings inside meta output: §9.4.
- M4 `identity` with `args: [--foo, bar]` and `files: 'a\.txt'`: prints `--foo`, `bar`, `a.txt`, each on its own line (verbose block with `- duration: 0.03s`).

**MC (migrate-config, run in a git repo):**
- MC1 Input with a header (`# comment header`, `---`, blank) and a list root using `-   repo:`, `sha: v1.0.0`, `stages: [commit, "push", 'merge-commit', manual]` and `language: python_venv`. The output keeps the header lines, inserts `repos:` after them, and gives `rev: v1.0.0`, `stages: [pre-commit, "pre-push", 'pre-merge-commit', manual]` and `language: python`. It prints `Configuration has been migrated.` (exit 0). A second run prints `Configuration is already migrated.`
- MC2 A flow list root `[{repo: local, hooks: [{…, language: "python_venv", stages: ['commit']}]}]` becomes `repos:\n    [{repo: local, hooks: [{…, language: "python", stages: ['pre-commit']}]}]`. The 4-space indent fallback is used.
- MC3 Invalid YAML: `An error has occurred: InvalidConfigError: \n==> File .pre-commit-config.yaml\n=====> while parsing a flow node\ndid not find expected node content\n  in "<unicode string>", line 5, column 1\nCheck the log at …`, exit 1. A missing file gives `An unexpected error has occurred: FileNotFoundError: [Errno 2] No such file or directory: '.pre-commit-config.yaml'`, exit 3. Outside git: `An error has occurred: FatalError: git failed. Is it installed, and are you in a Git repository directory?`, exit 1.
- MC4 Bugs: an aliased stages list gives `[pre-commitpre-commit]`, and a folded `language: >-\n  python_venv` gives `language: >python>`.
[[RECOVERY GAP: 4 original line(s) 1345-1348 could not be recovered after the /tmp wipe]]
## Addendum: Pick one regex engine using corpus evidence, and set Python `re` acceptance parity for config patterns

Status: gap-fill #5, 2026-10-08. **This addendum is normative.** Where it disagrees with an earlier part of this file or with another spec, this addendum wins.

### A.0 What this addendum replaces, and which contradictions it resolves

| Earlier text | Status after this addendum |
|---|---|
| 01 §13 (Python `re` semantics, acceptance matrix, "Recommendation for snp" (a)–(d)) | Facts still hold, with one correction: fancy's `\Z` means "end, ignoring *any number* of trailing newlines" (`a\Z` matches `a\n\n`), not "before the final newline". Recommendations (a)–(d) are **replaced** by §A.6. Recommendation (c)'s consuming `$` → `(?:\n?\z)` is unsafe when anything follows `$`: Python `a$\n` matches `a\n`, and the consuming form fails. Use §A.6.5 instead. |
| 01 §18.1 bullet `snp-pyre` | Kept as the crate name. Its contents are defined by §A.6. |
| 01 §19 Q7 (which CPython dialect) | **Decided:** CPython **3.12** grammar, with the Unicode data of `regex-syntax` (UCD 16.0, the same as CPython 3.14). See §A.3. |
| 04a §12.3 (pygrep bytes translator, "Tier 1 / Tier 2") | Two-tier idea kept. Three rules there were **wrong** and are corrected in §A.6.10: (1) fancy-regex `BytesMode::Ascii` cannot match bytes ≥ 0x80 written as escapes; (2) `(?i:…)` wrapping must be replaced by explicit ASCII folding; (3) `\z` must be rejected, because CPython 3.12 rejects it. A note was added at the top of 04a §12.3. |
| 07 D4 ("fancy-regex 0.19.2 as the single engine") and 07 §3.2 "Recommendations" | **Withdrawn** and replaced by the two-tier decision below. 07 D4 and 07 §3.2 now point here. |
| 10 Q6 ("decide after running the oracle corpus") | **Decided** (corpus run below). 10 Q6 now points here. |
| Contradiction "Regex engine" (01 §13/§18.1 + 04a §12.3 vs 07 D4 vs 10 Q6) | **Resolved: two tiers** (§A.1). |
| Contradiction "In-process emulation granularity", regex part only (04a §12.2 one pass vs 09 per partition vs 02 §21.14) | **Resolved for regex errors** (§A.4.5): the pattern is compiled once per run, and an invalid pattern is reported once. The file-processing granularity stays with 09. |

### A.1 Decision (TL;DR)

1. **Use two engines with one translator.**
   - **Tier 1** is the `regex` crate 1.13: `regex::Regex` for `str` patterns, and `regex::bytes::Regex` with `unicode(false)` for pygrep `bytes` patterns.
   - **Tier 2** is `fancy-regex` 0.19.2. It is used only when a pattern needs backtracking: look-around, backreferences, conditionals, atomic groups or possessive quantifiers.
   - **Both tiers get their input from the same snp-owned Python-`re` parser and lowering.** Neither engine ever parses user text directly.
2. **Corpus.** It holds 4,858 unique patterns:
   - 4,648 `str` patterns from `files`/`exclude`, taken from 2,388 real `.pre-commit-config.yaml` files (1,740 repos) and 1,083 real manifests, plus refs/pre-commit-hooks, refs/pre-commit and refs/prek;
   - 210 `bytes` patterns from pygrep `entry` fields, including the 10 pygrep-hooks.

   Tier 1 handles **99.1 %** of the valid `str` patterns (4,604 of 4,646) and 67.6 % of the pygrep patterns (142 of 210).
3. **Match parity of the translated pipeline: 100 %.** Both designs (two-tier and fancy-only) agree with CPython 3.12 `re.search` on all 4,644 comparable corpus patterns, over two path sets:
   - a pool of 72,388 paths (real, generated and adversarial);
   - each pattern's own repository tree (3.87 M real paths in total).

   They also agree on 18,342 of 18,382 random grammar-fuzzed patterns. All 40 residual differences are case-insensitive backreferences (§A.6.13).
4. **Raw engines are not acceptable.**
   - Raw `regex` rejects 49 valid corpus patterns.
   - Raw fancy-regex accepts all of them, and agrees on every real repository path. But it gives a different answer from Python on adversarial paths for **2,899 of 4,644** patterns, mostly through `$` before a trailing `\n`.
   - On the 509-entry edge list, at least one raw engine accepts or rejects differently from Python in **198** cases.
5. **Two-tier is faster than fancy-only.** Measured with rustc 1.98.0 over 100,000 real paths, all 4,646 patterns:

   | Design | ms per pattern per 100k paths |
   |---|---|
   | two-tier | 7.27 |
   | two-tier + relaxed prefilter | 5.38 |
   | fancy-only | 9.38 (+29 % over two-tier, +74 % over two-tier + prefilter) |
   | CPython (same 200-pattern sample) | 129 |

   For the 40 largest real configs (1,861 hooks), the time per config is:

   | Design | ms per config |
   |---|---|
   | CPython | 2,630 |
   | two-tier | 301 |
   | two-tier + prefilter | 234 |
   | fancy-only | 373 |
6. **The `regex` crate is mandatory even for "fancy-only".**
   - fancy-regex cannot express a byte ≥ 0x80 as an escape. Its `\xFF` means U+00FF, encoded as UTF-8.
   - pygrep therefore needs `regex::bytes` for byte-exact literals.
   - The sound prefilter that makes tier 2 faster than CPython is also a `regex` automaton.

   Since `regex` is in the dependency tree anyway, two-tier costs nothing extra.
7. **One translator spec serves config (`str`) and pygrep (`bytes`) patterns.**
   - A mode flag selects the str or bytes rules.
   - `$` and `\Z` follow one shared rule (§A.6.5). `\Z` always becomes `\z`. `$` (non-MULTILINE) always means `(?=\n?\z)`. Cheaper *equivalent* forms are allowed only when a precondition on the haystack is proven: no trailing `\n`, or at most one `\n` located at the end.
8. **Acceptance parity.**
   - snp accepts exactly what CPython **3.12** `re.compile` accepts.
   - Python's `OverflowError` (repeat count ≥ 4294967295), `ValueError` (for example `(?a)(?u)`) and `RecursionError` (≥ 485 nested groups) make pre-commit **crash with exit 3**. snp instead reports the clean validation error `'<repr>' is not a valid python regex` with exit 1, following the policy of 01 §16.
9. **Unicode semantics are exact for config patterns**, by explicit lowering:
   - `\w` becomes `[\p{L}\p{N}_]`. This is identical to Python's `\w` on UCD 16; Rust's `\w` differs on 3,557 code points.
   - `\s` becomes `[\s\x{1C}-\x{1F}]`.
   - `\d` stays Rust's `\d` (Nd).
   - IGNORECASE becomes explicit classes: Unicode simple folding plus the Turkish-i rule `{I, i, İ, ı}`.
   - `(?a)` becomes ASCII classes and ASCII folding.
10. **Tier 2 safety.**
    - Raise fancy's `backtrack_limit` to 100,000,000. A real pygrep pattern needed more than 10⁷ backtracks on a 3,377-byte line; the 1,000,000 default made it fail.
    - Always run the relaxed tier-1 prefilter first (§A.6.11). It is 6.3× faster on config look-ahead patterns and 40–1,200× faster on pygrep look-around patterns.

### A.2 Evidence

Everything lives under `scratchpad/rxgap/`:
- scripts in `scripts/`;
- data in `data/` (downloaded configs, manifests and trees: untrusted, parsed only), `corpus/` and `paths/`;
- results in `results/`;
- the Rust probe in `rxcorpus/` (regex =1.13.1, regex-syntax =0.8.11, regex-automata =0.4.18, fancy-regex =0.19.2, built with rustc 1.98.0 and 1.95.0, same results).

Python oracles:
- CPython 3.12.3 (`/usr/bin/python3.12`, UCD 15.0);
- CPython 3.10.18, 3.11.13, 3.13.14 and 3.14.0rc1, for version deltas;
- pre-commit 4.6.2 (`scratchpad/venv-pc462`) for end-to-end behaviour.

Every Python process ran with `-I`.

#### A.2.1 Corpus

| Source | Files | Pattern rows |
|---|---|---|
| GitHub repo search, stars-sorted (`stars:>20000`; `language:Python stars:>2000`; top 200 each of TS/JS/Go/Rust/C++/Java/Shell/Jupyter) → `.pre-commit-config.yaml` at HEAD | 407 configs (of 2,832 distinct repos searched) | 1,664 |
| GitHub code search `filename:.pre-commit-config.yaml` + `exclude` / `files` / `verbose` / `pygrep` | 1,985 configs | 4,462 |
| GitHub code search `filename:.pre-commit-hooks.yaml` + `files` / `exclude` / `pygrep` | 1,083 manifests | 1,749 |
| refs/pre-commit-hooks, refs/pre-commit (YAML and the Python test AST), refs/prek (YAML snippets in `.rs`/`.md`), refs/identify, the pygrep-hooks manifest | — | 146 |
| **Total** | 2,388 configs (1,748 with ≥ 1 pattern, 1,740 repos) + 1,083 manifests | 8,021 rows → **4,858 unique** (4,648 `str` + 210 `bytes`) |

Where the rows sit: `config.hook.files` 2,784; `config.hook.exclude` 2,610; `manifest.hook.files` 1,361; `top.exclude` 520; `manifest.hook.exclude` 309; pygrep `entry` 305; `top.files` 15.

Feature census over valid unique patterns (from CPython 3.12's own AST):

| Feature | `str` (4,646) | `bytes` (210) |
|---|---|---|
| non-MULTILINE `$` | **2,928 (63 %)** | 26 (by-line ⇒ exact `(?m:$)`) |
| `(?x)` verbose, leading | **571 (12 %)** | 30 |
| a newline inside the pattern (block scalars) | 541 | 12 |
| look-ahead / look-behind | 38 / 5 | 47 / 17 |
| backreference / conditional / atomic / possessive | 0 / 0 / 0 / 0 | 9 / 0 / 0 / 21 |
| `\w \W \d \D \s \S` | 42 | 69 |
| `\b` / `\B` | 7 / 0 | 46 |
| IGNORECASE (global or scoped) | 16 | 14 |
| lazy quantifiers | 9 | 23 |
| an unescaped literal `{` (`{{cookiecutter…}}`, `\.{js,ts}`) | 7 | 2 |
| `[[` (FutureWarning) | 1 | 1 |
| an empty alternative (`(a|)`, `|)`) | 22 | 4 |
| non-ASCII literal characters | 4 | 12 (7 of them in tier-2 patterns) |
| `\N{…}`, octal escapes, `\0`, `(?a)` in `str`, surrogate escapes, `\z` | 0 | 0 (one `(?a)`) |
| maximum repeat count / AST nesting depth / pattern length | 111 / 8 / **39,964 chars** | — |

**Tier split (translated).**
- `str`: 4,604 tier 1 and 42 tier 2.
- Of the tier-1 `str` patterns, 2,891 carry the "no trailing `\n`" precondition for `$` and 7 carry the "ASCII haystack" precondition for `\b`.
- `bytes`: 142 tier 1 and 68 tier 2.

#### A.2.2 Acceptance parity: raw engines vs CPython 3.12 `re.compile`

Corpus:

| Set | Python OK | raw `regex` disagrees | raw `fancy-regex` disagrees |
|---|---|---|---|
| `str` (4,648) | 4,646 (rejects `Some(`, a prek negative test, and the OCA-style exclude whose `# NOT INSTALLABLE ADDONS …` lines come *before* `(?x)`: `global flags not at the start of the expression`) | rejects **49** valid ones (42 look-around, 4 `repetition quantifier expects a valid decimal`, 3 `repetition operator missing expression`, all literal `{`); accepts the invalid OCA pattern | accepts the invalid OCA pattern; otherwise agrees |
| `bytes` (210) | 210 (3.10 rejects 21, which use possessive `*+`) | rejects **73**: look-around 58; backrefs 8; escapes 3 (`\Z`, and `\𝐄`, a backslash followed by a non-ASCII character, which Python reads as a literal); flags 2; class 1; quantifier 1 | rejects 2 (`(?a…)` "Unknown group flag"; a non-ASCII class, "error parsing pattern 0") |

Edge list (509 hand-written cases, `scripts/edge_cases.py`; results in `results/edge.py3{10..14}.jsonl` and `results/edge.rust_raw.jsonl`). One engine or both disagree with Python in **198** cases. The table below has a representative row for each kind of disagreement. "OK" means compiles.

| Pattern | Py 3.12 | regex | fancy | Note |
|---|---|---|---|---|
| `x{,}` `x{}` `x{` `x{1,2` `{` `{x}` `a{0x10}` `a{٣}` `{{cookiecutter.project_name}}` `\.{js,ts}$` | OK (literal `{` unless `{m}`, `{m,}`, `{,n}`, `{m,n}` with ASCII digits) | ERR | OK | |
| `{1}` `{,}` | ERR `nothing to repeat` | ERR | **OK** | |
| `x{2,1}` | ERR `min repeat greater than max repeat` | ERR | **OK** | |
| `x{2}{3}` `x*{2}` | ERR `multiple repeat` | **OK** | **OK** | |
| `x**` | ERR `multiple repeat` | **OK** | ERR | |
| `^*` `$+` `\b*` `\A?` | ERR `nothing to repeat` | **OK** | **OK** | |
| `(?=a)*` `(?!a)+` `(?<=a)?` `(?:)*` `(?:)+` | OK | ERR | **ERR** ("Target of repeat operator is invalid") | |
| `\0` `\07` `\123` `\377` `[\1]` `[\0]` | OK (octal) | ERR | ERR | |
| `\uD800` `[\ud800-\udfff]` `[^\ud800]` | OK | ERR | ERR | |
| `\N{EM DASH}` `\N{LATIN SMALL LETTER GHA}` (alias) `\N{CJK UNIFIED IDEOGRAPH-4E00}` | OK | ERR | OK but **wrong**: fancy reads `\N` as "not newline" followed by literal `{EM DASH}` | |
| `\N{NOPE}` `\N` `\N{}` | ERR | ERR | **OK** (same misreading) | |
| `\z` | ERR `bad escape \z` (OK on 3.14) | OK | OK | |
| `\Z` | OK (absolute end) | ERR | OK but means "end, ignoring *any number* of trailing newlines" (`EndTextIgnoreTrailingNewlines`) | |
| `\e` `\h` `\G` `\K` `\R` `\O` `\H` | ERR `bad escape` | ERR | **OK** | |
| `(?<a>x)` | ERR `unknown extension ?<a` (all of 3.10–3.14) | OK | OK | |
| `(?P<1a>x)` `(?P<a>x)(?P<a>y)` `(a\1)` | ERR | ERR | **OK** | |
| `(?<=a|bc)` `(?<=a*)` `(?<=a{1,2})` | ERR `look-behind requires fixed-width pattern` (compile phase, no position) | ERR | **OK** (variable look-behind) | |
| `(?<=(a)\1)` | ERR `cannot refer to group defined in the same lookbehind subpattern` | ERR | **OK** | |
| `(?(1)a|b)` `(a)(?(1)b|c|d)` `(?(0)a)` `(?(-1)a)` `(?(+1)a)(b)` | ERR | ERR | **OK** | |
| `a(?i)` `(a)(?i)` `(?i)|(?m)` `(?:(?i)a)` | ERR `global flags not at the start of the expression` (3.11+) | OK | OK | |
| `(?i-i:a)` `(?i-:a)` | ERR | ERR | **OK** | |
| `(?a)` `(?a:x)` `(?a)\w` `(?ai)k` `(?imsxa)a` | OK | ERR | ERR | |
| `(?t)a` | OK (3.10–3.12; removed in 3.13) | ERR | ERR | |
| `(?-u:x)` | ERR `cannot turn off flags 'a', 'u' and 'L'` | OK | ERR | |
| `[[]` `[[:alpha:]]` | OK + FutureWarning | ERR / **OK** (POSIX class) | ERR / OK (POSIX) | |
| `[a--b]` | ERR `bad character range a--` | OK (set difference) | OK | |
| `[\b]` | OK (backspace) | ERR | OK | |
| `[\B]` `[\A]` `[\z]` `[\e]` | ERR `bad escape` | ERR | **OK** | |
| `(?x)[ ]` `(?x)[#]` | OK (whitespace and `#` are literal inside a class) | ERR | OK | |
| `(?x)( ?:a)` | ERR `nothing to repeat` | OK | OK | |
| n nested groups, `(`×n `a` `)`×n or `(?:`×n … | OK for n ≤ 484 under pre-commit (RecursionError at 485) | OK for n ≤ 250, ERR at 251 (`exceed the maximum number of nested parentheses`, default `nest_limit` 250); with `nest_limit(2048)`, OK up to n = 1000 including quantifiers | **ERR for n ≥ 64** (`Pattern too deeply nested`; `MAX_RECURSION = 64`, `fancy-regex-0.19.2/src/lib.rs:142`, `parse.rs:968`) | |
| `a{4294967294}` | OK | ERR (size limit) | ERR | |
| bytes `[é]` `[\x80-\xff]` | OK | `[é]` ERR, `[\x80-\xff]` OK | ERR | |
| bytes `\u0041`, `\U00000041`, `(?P<é>x)`, `(?u)x` | ERR | OK | OK / ERR | |

#### A.2.3 Match parity

**Method.**
- **Python reference:** `re.search(p, path)` (CPython 3.12) for every (pattern, path).
- **Rust:** `rxcorpus match`, comparing (count, FNV-1a-64 over the matching indices) for four designs:
  - two-tier (translated);
  - fancy-only (the same translation, with every variant compiled by fancy);
  - raw fancy (prek-style; no translation);
  - raw `regex`.
- **Path sets:**
  - `pool` (72,388): 132 adversarial paths (trailing `\n`, `\r`, NFD, Turkish i, ß/ẞ, Kelvin K, ſ, U+0345, superscripts, Indic marks, `\x1c`, NBSP, U+2028, emoji, `{{…}}`, `[[…]]`, backslash, deep paths, 300-char paths); 51,244 strings generated from each pattern's AST plus variants (trailing `\n`, upper/lower case, i→ı, e→e+U+0301); 20,000 sampled real paths; 1,193 refs paths.
  - Each pattern's own repo tree, for the 3,683 patterns that came from a fetched config (1,740 trees, 3,866,591 paths, 16,695 of them non-ASCII).

`str` results (`results/pymatch.jsonl` vs `results/rustmatch.jsonl`):

| Design | Exact | Mismatch | Cannot compile |
|---|---|---|---|
| two-tier (translated) | **4,644 / 4,644** | 0 | 0 |
| fancy-only (translated) | **4,644 / 4,644** | 0 | 0 |
| raw fancy | 1,745 | 2,899 (`$` + trailing `\n` 2,892; `\w`/`\s` Unicode 4; Turkish i 2; `[[` 1) | 0 |
| raw regex | 1,731 | 2,864 | 49 |
| raw fancy on **own repo trees only** | 3,683 / 3,683 | 0 | — |
| raw regex on own repo trees only | 3,640 | 0 | 43 |

Two patterns could not get a Python reference: `^(.+/)*\.pre-commit-config\.yaml$` and `^(.+/)*\.pre-commit-hooks\.yaml$`. CPython did not finish within 600 s on the pool. This is exponential backtracking: 0.074 s for a path with 20 segments, doubling with each extra segment, so about 75 s at 30 segments. **pre-commit itself hangs on such paths.** snp tier 1 is linear, which is a strictly better and otherwise unobservable difference.

Random-grammar fuzz (`scripts/fuzz_gen.py`): 20,000 patterns, of which CPython accepts 18,382, over 4,329 haystacks of length 1–7 built from 36 tricky code points.

| Design | Exact | Mismatch | Cannot compile |
|---|---|---|---|
| two-tier | **18,342** | 40 (**all** are IGNORECASE + backreference, §A.6.13 D1) | 0 |
| fancy-only | 18,342 | 40 (same) | 0 |
| raw fancy | 9,709 | 2,453 | 6,220 (`(?a…)` 3,887; "Target of repeat operator is invalid" 2,324; `\b{` 9) |
| raw regex | 7,091 | 1,860 | 9,431 |

The first fuzz pass also found three translator bugs, fixed before these numbers were taken:
- `(?i)` must not wrap categories, because Rust folding adds U+0345 to `\p{L}`;
- zero-width items under nested quantifiers;
- IGNORECASE backreferences must be emitted as `(?i:\N)`.

`bytes` (pygrep) results: 210 patterns over 189,166 lines (refs sources, adversarial lines, generated positives), plus 1,183 whole files for the 5 `--multiline` entries.

| Design | Exact | Notes |
|---|---|---|
| two-tier / fancy-only (translated) | **207** | 2 "mismatches" are artefacts: synthetic lines containing an **interior** `\n`, which cannot occur in by-line mode. 1 fails with the default backtrack limit on a 3,377-byte line: `(?<!#).*(?<!\$)\b/tmp/` needs > 10⁷ and ≤ 10⁸ backtracks. |
| raw fancy `BytesMode::Ascii` | 188 | 18 mismatches, 2 compile errors, 2 runtime errors |
| raw `regex::bytes` | 130 | 73 compile errors, 7 mismatches |

#### A.2.4 Throughput and memory

All runs use rustc 1.98.0 `--release` (`lto="thin"`, `codegen-units=1`) on an idle 22-core Linux box. The paths are 100,000 real ones (6.48 MB) sampled from the 1,740 trees (`paths/bench100k.paths`). `is_match` is called per (pattern, path).

| Measurement | two-tier | two-tier + prefilter | fancy-only | raw fancy | CPython 3.12 |
|---|---|---|---|---|---|
| all 4,646 `str` patterns: ms per pattern per 100k paths | **7.27** | **5.38** | 9.38 | 9.49 | — |
| 200-pattern random sample (same patterns) | 6.05 | 5.19 | 7.50 | 6.46 | **129.0** |
| the 42 tier-2 `str` patterns | 228 | **36.1** | 233 | 277 | 73.5 |
| compile, all 4,646 (ms total) | 986 | 925 | 938 | 605 | (≈ same order; 42 ms for the 39,964-char pattern) |
| 40 largest real configs (1,861 hooks) through pre-commit's filter algorithm (top-level filter, then each hook's `files`/`exclude`): ms per config | 301 | **234** | 373 | 359 | **2,630** |
| same, with `regex::RegexSet` over each config's distinct tier-1 patterns | 285 | — | — | — | — |
| RSS per compiled pattern (1,000 patterns; exact variant built lazily) | 25.4 KiB | — | 29.2 KiB | 28.4 KiB | — |
| RSS per pattern if exact variants are built eagerly | 42.1 KiB | — | 42.4 KiB | — | — |

pygrep tier 2 over 189,166 lines (`rxbytes`):

| Pattern | fancy alone | fancy `seek(true)` (experimental) | relaxed prefilter + fancy | CPython |
|---|---|---|---|---|
| `(?<!#).*(?<!\$)\b/tmp/` | 26.5 s (11.1 s on raw bytes with `BytesMode::Ascii`) | 28 ms | **22 ms** (49 candidates) | 3.76 s |
| `(?i)# noqa(?!: )` | 301 ms | 12.7 ms | **7.4 ms** (42 candidates) | 20 ms |

Conclusions:
- fancy's backtracking VM is 3–7× *slower* than CPython's sre on hard patterns. A tier-1 prefilter is therefore mandatory, not optional.
- Wrapping tier-1 patterns in fancy costs about 29 % with no benefit.

#### A.2.5 Structural constraints that decide the design

| Constraint | Evidence | Consequence |
|---|---|---|
| fancy-regex cannot express a byte ≥ 0x80. In `BytesMode::Ascii`, `\xFF` and `\x{FF}` become the *character* U+00FF and match only `C3 BF`; a class `[\x80-\xFF]` gives `error parsing pattern 0`. | `rxbytes` probe: `\xFF` vs `b"\xff"` → None; vs `"ÿ"` → (0,2) | pygrep needs `regex::bytes`, or the Latin-1 lane (§A.6.10), for any non-ASCII byte. 7 corpus pygrep patterns are tier 2 *and* contain non-ASCII bytes. |
| In `regex::bytes` with `unicode(false)`, braced `\x{FF}` means U+00FF in UTF-8; only `\xFF` (two digits) means the byte. Braced escapes inside classes give "Unicode not allowed here". | probe | Bytes mode emits `\xHH` (2 digits) and never `\x{…}`. |
| fancy-regex nesting is capped at 63 groups (hard-coded); regex `nest_limit` defaults to 250 (`regex-syntax-0.8.11/src/ast/parse.rs:141`). Python accepts up to 484. | nesting probe (`results/nest.jsonl`) | Tier 1 must use `nest_limit(2048)` (verified up to 1,000 nested quantified groups). A tier-2 pattern, or an exact variant, nested ≥ 64 deep is a documented limitation (D6). |
| fancy rejects inline `(?-u:…)` / `(?u)` changes ("Changing Unicode mode inline is not supported"). | sem probe | ASCII `\b` in tier 2 is emitted as look-around, never as `(?-u:\b)`. |
| fancy `\Z` = end, ignoring all trailing newlines; fancy `\N` = not newline; fancy `\<` `\>` = word boundaries; fancy `{n}+` = possessive; in regex `a{2}+` is a nested repeat and `a++` is also a nested repeat (different semantics). | probes | The emitter never produces `\Z`, `\N`, `\<`, `\>`. Possessive quantifiers go only to fancy. |
| fancy case-insensitive backrefs compare by simple folding **and** require equal UTF-8 byte length (`fancy-regex-0.19.2/src/vm.rs:849-920`). | fuzz D1 | Residual divergence D1. |
| fancy `is_match` is fallible (`Result`) even for delegated patterns, and its `RegexSet` uses earliest-position semantics. | API | Tier 1 keeps an infallible `bool` API. `regex::RegexSet` stays available for a per-path multi-pattern pass. |

#### A.2.6 Decision matrix

| Criterion | two-tier | fancy-only |
|---|---|---|
| Match parity (corpus + fuzz) | 100 % (except D1) | 100 % (except D1) |
| Throughput, tier 1 | **baseline** | +29 % time |
| Throughput, tier 2 | prefilter built from `regex` | needs `regex` anyway for the prefilter, or experimental `seek` |
| pygrep byte literals ≥ 0x80 | `regex::bytes` | impossible without the Latin-1 lane, which needs a copy for every haystack |
| Nesting > 63 | OK in tier 1 | ERR |
| API | `bool` for 99 % of patterns | `Result<bool>` everywhere |
| Dependencies | `regex` + `fancy-regex` (fancy depends on `regex-automata` / `regex-syntax` anyway) | the same, because `regex` is still needed (bytes, prefilter, internal fixed patterns, per 07) |
| Code size | two backends behind one enum (~150 lines) | one backend |

**Verdict: two-tier.**

### A.3 Which Python: acceptance differences across CPython 3.10–3.14 (measured)

pre-commit 4.6.2 runs on whichever Python the user installed (≥ 3.10). So the accept/reject answer for a config pattern **depends on the user's Python**. The table below covers every difference found: the 509-entry edge list, the corpus, and a diff of `sre_parse.py` / `re/_parser.py` across versions.

| Construct | 3.10 | 3.11 | 3.12 | 3.13 | 3.14 | Source |
|---|---|---|---|---|---|---|
| possessive `*+ ++ ?+ {m,n}+` | ERR `multiple repeat` | OK | OK | OK | OK | `re/_parser.py:697-701` (3.12) |
| atomic `(?>…)` | ERR `unknown extension ?>` | OK | OK | OK | OK | `_parser.py:841-844` |
| global flags not at the start (`a(?i)`, `(?:(?i)a)`, `(?i)a|(?m)b`, OCA-style `# comment\n(?x)…`) | OK + DeprecationWarning (hidden) and a **re-parse in verbose mode** if `(?x)` | ERR `global flags not at the start of the expression` | ERR | ERR | ERR | `_parser.py:838-842` |
| conditional group name `+1`, `١` (non-ASCII digit) | OK | OK + DeprecationWarning | ERR `bad character in group name '+1'` | ERR | ERR | `_parser.py:791-816` |
| bytes group name non-ASCII `(?P<é>x)` | ERR (`'Ã©'`) | ERR (`'\xc3\xa9'`) | ERR | ERR | ERR | `_parser.py:304-310` |
| `(?t)` TEMPLATE flag | OK (`(?t)a*` → `internal: unsupported template operator MAX_REPEAT`) | same | same | ERR `unknown extension ?t` | ERR | `_parser.py:63`, `_compiler.py:104-105` |
| `\z` | ERR `bad escape \z` | ERR | ERR | ERR | **OK** (= `\Z`) | 3.14 `CATEGORIES` |
| `(?!)` | ASSERT_NOT | same | same | compiled as `FAILURE` (same semantics) | same | 3.13 `_parser.py:782-785` |
| `\N{GARAY CAPITAL LETTER A}` (UCD 16) | ERR | ERR | ERR `undefined character name …` | ERR | OK | `unicodedata` |
| Unicode data (affects `\w \d \s`, IGNORECASE, `\N{}`) | UCD 13.0 | 14.0 | 15.0 | 15.1 | 16.0 | `unicodedata.unidata_version` |

On the corpus, 3.11 through 3.14 accept and reject identically. 3.10 differs on 21 pygrep patterns (possessive) and on the OCA exclude (3.10 accepts it).

**Decision.**
- snp's grammar is **CPython 3.12**, which is what 01 Q7 proposed and what the 4.6.2 oracle venv runs.
- snp therefore accepts possessive and atomic constructs.
- It rejects `\z`, `(?<name>…)`, non-initial global flags, and non-ASCII-decimal conditional names.
- It accepts `(?t)` with 3.12's compile-time rule: the error `internal: unsupported template operator MAX_REPEAT|MIN_REPEAT|POSSESSIVE_REPEAT` (no position) when the pattern contains any repeat.
- Unicode **data** comes from `regex-syntax` 0.8.11 (UCD 16.0, i.e. CPython 3.14). This changes results only for code points that are unassigned in UCD 15.0 (Garay, Tulu-Tigalari, Latin additions U+A7CB–A7DC, …). Paths and lines with such characters are vanishingly rare, so do not ship a second set of Unicode tables just for 3.12.

### A.4 Error classes, and what pre-commit does with them

`cfgv.check_regex` catches **only** `re.error` (`cfgv/cfgv.py:384-388`). Every other exception escapes to pre-commit's error handler. The outputs below were observed with pre-commit 4.6.2 on CPython 3.12.3, running `validate-config` and `run --all-files` in a throwaway repo (`scratchpad/tmp-regex/`); `$PRE_COMMIT_HOME` was redacted.

| Trigger | Python exception | pre-commit output (exact) | Exit | snp behaviour (decided) |
|---|---|---|---|---|
| syntax error (`(?<a>x)`, `[`, `(`, `x**`, …) | `re.error` | `validate-config`: `\n==> File .pre-commit-config.yaml\n==> At Config()\n==> At key: repos\n==> At Repository(repo='local')\n==> At key: hooks\n==> At Hook(id='echo')\n==> At key: files\n=====> '(?<a>x)' is not a valid python regex`. `run`: the same block, prefixed by `An error has occurred: InvalidConfigError: ` and followed by `Check the log at $PRE_COMMIT_HOME/pre-commit.log` | 1 | identical (01 §7/§8) |
| compile-phase error: `(?<=a|bc)`, `(?<=a*)`, `(?t)a*`, look-behind width > MAXCODE | `re.error`, no position | same as above | 1 | identical. The snp parser must compute look-behind widths (§A.6.2 R-LB). |
| repeat count ≥ 4294967295 (`a{4294967295}`, `a{1,4294967295}`, `a{,4294967295}`, `a{99999999999999999999}`) | `OverflowError: the repetition number is too large` (`_parser.py:671,675`) | `An unexpected error has occurred: OverflowError: the repetition number is too large` / `Check the log at $PRE_COMMIT_HOME/pre-commit.log` | **3** | the validation error `'a{4294967295}' is not a valid python regex`, exit 1 (01 §16 policy). Add this row to the 01 §16 table. |
| incompatible type flags in separate groups: `(?a)(?u)x` (str); `(?a)(?L)x` (bytes) | `ValueError: ASCII and UNICODE flags are incompatible` / `ASCII and LOCALE flags are incompatible` (`_parser.py:953-967`) | `An unexpected error has occurred: ValueError: ASCII and UNICODE flags are incompatible` + the log line | **3** | the validation error, exit 1 (add to 01 §16) |
| group nesting ≥ 485 (`(`×485 … or `(?:`×485 …) | `RecursionError: maximum recursion depth exceeded` | `An unexpected error has occurred: RecursionError: maximum recursion depth exceeded` + the log line | **3** | Accept nesting ≤ 484, which is exactly pre-commit's limit: measured by bisection for both capturing and non-capturing groups, under both `validate-config` and `run`. At ≥ 485, give the validation error, exit 1. The parser must not recurse natively beyond its own explicit limit. |
| `[[` / `[a&&b]` / `[a~~b]` / `[a||b]` / `[---]` (`FutureWarning`, `_parser.py:556-606`) | — (a warning, not an error) | on **stderr**, once per process: `<site-packages>/cfgv.py:386: FutureWarning: Possible nested set at position 1` followed by `  re.compile(v)`. Then normal output; exit unchanged. | 0 | Do **not** replicate the Python file path (Tier B). Print once per distinct pattern, on stderr: `[WARNING] regex {pattern!r}: Possible nested set at position 1 (Python FutureWarning)`. Behind `SNP_PYTHON_WARNINGS=0` it is silent. The match semantics are Python's (a literal `[`, `&`, `~`, `|`). |
| DeprecationWarnings (3.10 flags-not-at-start; 3.11 bad conditional names) | — | not shown (the default filter hides DeprecationWarning outside `__main__`) | — | irrelevant: these are errors under 3.12 |

**Error-message format** (needed for pygrep, §A.4.5), from `re/_constants.py` `error.__init__`:
- `str(e)` is `"{msg} at position {pos}"`.
- If the pattern contains `\n`, append `" (line {L}, column {C})"`. `L` = number of `\n` in `pattern[:pos]` + 1; `C` = `pos - pattern.rfind('\n', 0, pos)`.
- Compile-phase errors have `pos=None`, so the bare message is used.
- `pos` counts **characters** for `str` patterns and **bytes** for `bytes` patterns.
- For `bytes` patterns the message is `msg.encode('ascii','backslashreplace')`, for example `bad character in group name '\xc3\xa9'`.

Example: `missing ), unterminated subpattern at position 11 (line 3, column 3)`.

#### A.4.5 pygrep: how regex errors surface (evidence for the "granularity" contradiction)

pre-commit runs `python -m pre_commit.languages.pygrep` **once per xargs partition**. An invalid entry therefore prints one full traceback **per partition**.

Measured in a repo of 40,000 files: 19 partitions, so 19 tracebacks; `grep….Failed`, `- hook id: grep`, `- exit code: 1`. The last line of each traceback is:
- `re.error: missing ), unterminated subpattern at position 0`;
- or `OverflowError: the repetition number is too large`;
- or `re.error: look-behind requires fixed-width pattern`.

The invalid pattern is **not** caught at config validation, because `entry` is not a validated regex.

**snp rule.**
- Translate and compile the pygrep pattern **once per hook per run**, before partitioning.
- If it is invalid, the hook fails with exit code 1. The output is one line in the exact Python form (`re.error: {str(e)}`, `OverflowError: the repetition number is too large`, `ValueError: …`, `RecursionError: maximum recursion depth exceeded`), followed by `\n`.
- The tracebacks themselves cannot be byte-identical anyway, because they contain interpreter paths. The repetition per partition is therefore a declared **Tier B** difference.
- For readable files, partition semantics stay as 09 decided.
- 04a §12.2's "one pass" is to be read as "compile once". It does not mean a different partitioning of file processing.

### A.5 Unicode-`str` semantics: Python vs Rust (exhaustively measured over all 1,114,112 code points)

`results/pyclasses.py3{10..14}.json` vs `results/rust_classes.jsonl`, compared with `scripts/cmp_classes.py`.

#### A.5.1 Character classes (str patterns, no `(?a)`)

| Python | Definition in CPython | Exactly equal in Rust (UCD 16) | Rust's own `\w`/`\s`/`\d` vs Python |
|---|---|---|---|
| `\w` | `ch.isalnum() or ch == '_'`, i.e. categories L* ∪ N* (Nd, Nl, No) ∪ `_` (`SRE_UNI_IS_WORD`) | `[\p{L}\p{N}_]`: **0 differences** vs 3.14. vs 3.12 the only differences are code points unassigned in UCD 15.0. | Rust `\w` = Alphabetic + M + Nd + Pc + Join_Control. **915** code points are Python-only (No: `² ³ ¹ ¼ ½ ¾ ①…`); **2,642** are Rust-only (Mn/Mc/Me combining marks such as U+0301, Pc such as `‿`, ZWJ/ZWNJ, Other_Alphabetic such as Ⓐ). So `^\w+$` vs `हिन्दी` is **False** in Python and True in Rust. |
| `\d` | `Py_UNICODE_ISDECIMAL` = Nd | `\d` (= `\p{Nd}`): 0 differences vs 3.14 | same as Rust's `\d` |
| `\s` | `Py_UNICODE_ISSPACE`: `\t\n\v\f\r`, **`\x1c`–`\x1f`**, space, U+0085, U+00A0, U+1680, U+2000–200A, U+2028, U+2029, U+202F, U+205F, U+3000 | `[\s\x{1C}-\x{1F}]`: 0 differences at any version | Rust `\s` (White_Space) lacks U+001C–U+001F |
| `\W \D \S` | complements | `[^\p{L}\p{N}_]`, `\D`, `[^\s\x{1C}-\x{1F}]` | — |
| `.` | any code point except `\n` (DOTALL: any) | `.` / `(?s:.)` | same |
| `\b` / `\B` | boundary by the `\w` above. **On an empty string both are False** (`SRE_AT_BOUNDARY` / `SRE_AT_NON_BOUNDARY` return 0 when `beginning == end`). | exact: look-around with `[\p{L}\p{N}_]` (§A.6.5) | Rust `\b` uses Rust `\w`. Rust `\B` is **True** on `""`. On ASCII-only haystacks both agree with Python. |

#### A.5.2 `(?a)` ASCII flag (str) and `bytes` patterns

| Python | `(?a)` str / bytes | Emit |
|---|---|---|
| `\w` | `[0-9A-Za-z_]` | `[0-9A-Za-z_]` |
| `\d` | `[0-9]` | `[0-9]` |
| `\s` | `[\t\n\v\f\r ]` (`Py_ISSPACE`, **no** `\x1c`–`\x1f`) | `[\x09-\x0D\x20]` |
| `\b` `\B` | ASCII word boundary (false on the empty haystack) | regex tier: `\b`/`\B` (bytes, `unicode(false)`); str: `\b` on ASCII haystacks, look-around otherwise |
| IGNORECASE | ASCII letters only: `(?ai)k` does **not** match U+212A; `(?ai)é` matches only `é` | explicit `[kK]`, never `(?i)` |
| `(?L)` (bytes only) | locale-dependent. In the C and UTF-8 locales, no byte ≥ 0x80 is alnum or cased. | treat as ASCII. Document it (§A.6.13 D4). |

#### A.5.3 IGNORECASE (str, Unicode)

**Python's rule** (`_compiler.py:40-97`, `_casefix.py`):
- A literal `c` with `iscased(c)` False matches only itself.
- Otherwise `d` matches if `lower(d) ∈ {lower(c)} ∪ _EXTRA_CASES[lower(c)]`.
- `lower` is `_sre.unicode_tolower`: the simple mapping, or the first character of the full mapping, so `lower('İ') = 'i'`.
- `iscased(c)` is `c != lower(c) or c != upper(c)`.
- `_EXTRA_CASES` has 50 pairs (`i↔ı`, `s↔ſ`, `µ↔μ`, `ι↔ͅ↔ι`, `β↔ϐ`, `ς↔σ`, …).
- Classes use the same `lower(d) ∈ charmap` rule over all members, but only if at least one member is cased.

This model was verified exhaustively: 0 mismatches on 2,981 cased code points, on every version from 3.10 to 3.14.

**Rust's rule**: Unicode *simple case folding* orbits (CaseFolding.txt, status C+S).

**Difference at equal Unicode version (3.14 vs regex-syntax): only the Turkish i family.**
- Python `(?i)i` and `(?i)I` also match `İ` (U+0130) and `ı` (U+0131).
- `(?i)İ` and `(?i)ı` match `I`, `i` and each other.
- Rust keeps `{I, i}`, `{İ}` and `{ı}` separate.

Against 3.12, Rust additionally folds 54 code points that UCD 15.0 lacks.

**Emission** (§A.6.6):
- expand literals and class members to explicit classes: `fold_simple(set)`;
- if the result intersects `{U+0049, U+0069, U+0130, U+0131}`, add all four;
- **do not** fold `\w \d \s` categories. They are closed under Python's lowering, and applying Rust `(?i)` to `\p{L}` wrongly adds U+0345 (found by the fuzzer: `(?i)\w` vs `ͅ` is False in Python).

Backreferences under IGNORECASE compare `lower(a) == lower(b)` per character, **without** `_EXTRA_CASES`: `(?i)(s)\1` vs `sſ` → False; `(?i)(k)\1` vs `kK` → True. fancy cannot reproduce this exactly (D1).

#### A.5.4 Other Unicode points

- **Surrogates.** In str mode, `\uD800`–`\uDFFF` and `\U0000D800`–`\U0000DFFF` are legal Python pattern characters. They can never occur in a path decoded strictly from UTF-8: pre-commit decodes git output strictly and crashes on invalid UTF-8.
  - A literal surrogate is lowered to the never-matching class `[^\x{0}-\x{10FFFF}]`.
  - Class ranges are clipped to exclude D800–DFFF.
  - An empty positive class becomes never-matching; an empty negated class becomes `(?s:.)`.

  Example: `[^\ud800]` matches every character including `\n`. This is verified.
- **`\N{name}`.** Python uses `unicodedata.lookup`, verified on 3.12:
  - Ordinary names and all 477 NameAliases.txt aliases (correction, control, alternate, figment, abbreviation: `LATIN SMALL LETTER GHA`, `LF`, `BOM`) are matched **ASCII-case-insensitively**.
  - The algorithmic `CJK UNIFIED IDEOGRAPH-XXXX` and `HANGUL SYLLABLE …` are matched **case-sensitively**: `CJK UNIFIED IDEOGRAPH-4e00` is rejected.
  - `CJK COMPATIBILITY IDEOGRAPH-F900`, `NUSHU CHARACTER-1B170` and `KHITAN SMALL SCRIPT CHARACTER-18B00` are case-insensitive.
  - `TANGUT IDEOGRAPH-17000` is **rejected** by 3.12.
  - Named sequences are rejected.
  - There is no loose matching: `LATIN_SMALL_LETTER_A`, a leading or trailing space, doubled spaces and `LATIN SMALL LETTER-A` are all rejected.

  `unicode_names2` 4.0.0 (UCD 16, `#![deny(unsafe_code)]`, about 1.3 MB of binary) finds every accepted name, but it also accepts the loose forms (UAX44-LM2). Its `name(c)` returns `None` for controls. Rule for snp:
  1. Look up with `unicode_names2::character(n)`.
  2. Accept only if one of these holds:
     - `n.to_ascii_uppercase()` equals the canonical `unicode_names2::name(c)`. For the `CJK UNIFIED IDEOGRAPH-` and `HANGUL SYLLABLE ` families, require `n` to be exactly uppercase.
     - `n.to_ascii_uppercase()` is in a generated static NameAliases table (477 entries).
  3. Reject `TANGUT IDEOGRAPH-…`.

  Golden vectors come from CPython: every assigned code point's name, its lowercase form, and its aliases. Corpus usage is 0, so put this behind a cargo feature `py-re-names` (default **on**). With the feature off, `\N{…}` is rejected with `\N{...} escapes are not supported in this build`. That is a declared deviation.
- **Normalization.** Neither Python nor snp normalizes. NFD `e`+U+0301 is two code points to both. On macOS, git's `core.precomposeUnicode` decides what the paths look like; snp must not "fix" them.

### A.6 The translator: one spec for config (`str`) and pygrep (`bytes`) patterns

Crate `snp-pyre`. There is no `unsafe`. Every entry point returns a `Result`, and no input may cause a panic.

#### A.6.1 Pipeline and public API

```
pattern text ──► Tokenizer ──► Parser (CPython 3.12 grammar, §A.6.2) ──► Ast + GroupTable + Flags + Warnings
                                                                              │
                                     Checks after the parse (group refs, fix_flags, look-behind widths, (?t))
                                                                              │
                                     Lowering (§A.6.4–A.6.9) ──► Lowered { tier, fast, exact, relaxed, cond }
                                                                              │
                        tier 1: regex::Regex / regex::bytes::Regex     tier 2 / exact: fancy_regex::Regex (+ Latin-1 lane)
```

```rust
pub enum Mode { Str, Bytes }                       // Str: files/exclude, builtin-hook --pattern; Bytes: pygrep entry
pub enum Haystack { Path, Line, Blob }             // Path: str paths; Line: pygrep by-line (≤ 1 '\n', at the end); Blob: whole file
pub struct Options {
    pub mode: Mode,
    pub haystack: Haystack,
    pub ignore_case: bool,                         // pygrep -i
    pub multiline_dotall: bool,                    // pygrep --multiline (re.M | re.S)
}
pub enum PyReError {
    Re { msg: Box<str>, pos: Option<usize> },      // == Python re.error; Display = Python str(e) (§A.4)
    Overflow,                                      // "the repetition number is too large"
    Value(Box<str>),                               // "ASCII and UNICODE flags are incompatible", …
    Recursion,                                     // nesting ≥ 485 groups
    Unsupported(Box<str>),                         // valid Python, impossible in snp (§A.6.13); never for the corpus
}
pub fn compile(src: &str, opts: Options) -> Result<PyRegex, PyReError>;
impl PyRegex {
    pub fn search(&self, h: &str) -> Result<bool, MatchError>;        // re.search(...) is not None
    pub fn match_prefix(&self, h: &str) -> Result<bool, MatchError>;  // re.match   → lower as \A(?:P)
    pub fn fullmatch(&self, h: &str) -> Result<bool, MatchError>;     // re.fullmatch → \A(?:P)\z
    pub fn search_bytes(&self, h: &[u8]) -> Result<Option<(usize, usize)>, MatchError>; // pygrep needs the span
    pub fn warnings(&self) -> &[PyWarning];                           // FutureWarning texts, §A.4
}
```

Notes on `Haystack` and `Mode`:
- `Haystack` matters only for `$` (§A.6.5).
- Config patterns use `Path`.
- `Mode::Bytes` uses a *bytes tokenizer*. The entry string is UTF-8-encoded, as pre-commit does with `args.pattern.encode()`. Each byte is then one "character", exactly as CPython decodes bytes patterns as Latin-1 (`_parser.py:229-236`). Error positions are byte offsets.

#### A.6.2 Parser: acceptance rules (CPython 3.12 `re/_parser.py`)

The parser is a port of the *grammar and checks*, not of the code. It must be iterative, or recursive with an explicit depth counter, so that nesting is limited to **484** groups (§A.4). It records, per error, the Python message and `pos`.

Notation:
- "tell" is the index (chars for str, bytes for bytes) of the next unread **token**.
- A token is one character, or `\` plus one following character.
- An escape token is read as a whole. So inside comments and classes, `\)` and `\]` never terminate anything.

**Tokenizer.**
- T1. A trailing lone `\` raises `bad escape (end of pattern)` at `pos = len-1` (`_parser.py:252`). It is raised **as soon as the tokenizer looks ahead at it**, even inside `(?#…` or `[…`. For example, `(?#a\`, `[a\`, `a{1\`, `(?P<a\` and `(?x)a#b\` all give this error, with pos = len-1 (verified).

**Sequence** (`_parse`, `_parser.py:512-886`). Loop over tokens until end, `|` or `)`.
- S1. In verbose mode (global or scoped `x`), skip `' ' \t \n \r \v \f` (exactly these 6, `_parser.py:26`). Also skip `#` up to and including the next `\n`, or up to the end. Verbose skipping happens **only between atoms**: never inside a class, `{…}`, a group name, a flag list, or between `(` and `?`.
  - `(?x)( ?:a)` → `nothing to repeat`.
  - `(?x)(? :a)` → `unknown extension ? `.
  - `(?x)a{ 2}` is literal `a{2}`, with the space skipped.
  - `(?x)a {2}` is `a{2}`.
  - `\ ` and `\#` are literals.
- S2. A token that is not in `.\[{()*+?^$|` is a LITERAL. This includes `]`, `}`, `-`, `#` (non-verbose) and any non-ASCII character.
- S3. `\`-escapes outside a class (`_escape`, `_parser.py:372-445`), in this order:
  - `\A \b \B \d \D \s \S \w \W \Z`: categories and anchors;
  - `\a \f \n \r \t \v \\`: literals. Outside a class `\b` is the word boundary, because CATEGORIES is checked first; inside a class it is backspace (S4);
  - `\x` + **exactly 2** hex digits, else `incomplete escape \x…` (pos = escape start);
  - str only: `\u` + exactly 4 hex, `\U` + exactly 8 hex. A value > 0x10FFFF gives `bad escape \U00110000`. Surrogates are allowed;
  - str only: `\N{name}` (§A.5.4). The errors are `missing {` (pos after `\N`), `missing character name`, `missing }, unterminated name`, `undefined character name 'X'`;
  - `\0` + up to 2 octal digits gives an octal literal;
  - `\[1-9]`:
    - if the next char is a digit, take it;
    - if both taken digits are octal and the next char is octal, it is a 3-digit **octal** literal. A value > 0o377 gives `octal escape value \477 outside of range 0-0o377`;
    - otherwise it is a **group reference** `N` (1–2 digits). If `N` exceeds the number of capturing groups opened so far: `invalid group reference N` at pos = escape start + 1. If the group is still open: `cannot refer to an open group`;
  - `\` + ASCII letter not listed above → `bad escape \X`;
  - `\` + any other char (punctuation, digits already handled, **non-ASCII letters such as `\é`**) → that literal.
  - Bytes mode: `\u`, `\U` and `\N` are `bad escape` (no `(?u)`).
- S4. `[` opens a class (`_parser.py:552-631`).
  - If the next token is `[`, record FutureWarning `Possible nested set at position {tell}`.
  - An optional `^` negates.
  - Items are read until `]`. A `]` that would be the first item is a literal.
  - The end of input gives `unterminated character set` at the `[`.
  - Escapes inside a class (`_class_escape`, `_parser.py:312-370`):
    - the ESCAPES set, where `\b` is backspace U+0008;
    - only the IN-categories `\d \D \s \S \w \W` (so `\A \B \Z \z` → `bad escape`);
    - `\x`, `\u`, `\U`, `\N` as in S3;
    - `\` + octal digit + up to 2 more octal digits is octal (never a backreference, so `[\1]` is U+0001);
    - `\8`/`\9` → `bad escape \8`;
    - other ASCII letters → `bad escape`;
    - anything else is a literal.
  - Range `a-b`: `-` followed by `]` means a literal `-` and the end of the class.
    - Either side being a category gives `bad character range \w-.` (the raw token texts), at pos = start of the left item.
    - `hi < lo` gives the same message.
    - A doubled `--`, `&&`, `~~`, `||` after at least one item records FutureWarning `Possible set difference|intersection|symmetric difference|union at position {i}`. These remain **literal characters**.
    - `[a--b]` is an error, because `-` (0x2D) < `a`.
- S5. Quantifiers `* + ? {` (`_parser.py:639-704`).
  - `{` followed by `}` is a literal `{`.
  - Otherwise read ASCII digits `lo`, optionally `,` and digits `hi`. If `}` does not follow, the `{` is a **literal** and parsing resumes right after it: `x{`, `x{a}`, `x{1,2`, `x{-1}`, `x{ 2}`, `a{٣}`, `{{cookiecutter.x}}` are all literal braces.
  - Empty `lo` means 0; `{n}` means `{n,n}`; an empty `hi` after `,` means unbounded. So `{,}` is `*` and `{,3}` is `{0,3}`.
  - A value ≥ 4294967295 raises **OverflowError**.
  - `hi < lo` gives `min repeat greater than max repeat`, at pos = index after `{`.
  - The quantifier target is the previous item:
    - none, or an anchor (`^ $ \A \Z \b \B`), gives `nothing to repeat` at the quantifier;
    - a quantified item gives `multiple repeat` at the quantifier.
  - Look-around assertions and groups **may** be quantified.
  - A trailing `?` makes the quantifier lazy; a trailing `+` makes it possessive.
- S6. `(`: groups and extensions (`_parser.py:708-873`).
  - `(?` at the end gives `unexpected end of pattern`.
  - `(?P<name>…)`: the name runs up to `>`. Errors: `missing group name`, `missing >, unterminated name`.
    - str: `name.isidentifier()`, i.e. XID_Start/`_` then XID_Continue; use the `unicode-ident` crate.
    - bytes: ASCII **and** identifier.
    - Otherwise `bad character in group name 'x'`; in bytes mode the name is shown with `%a`.
    - A duplicate name gives `redefinition of group name 'a' as group 2; was group 1`.
  - `(?P=name)`: a named backreference. Errors: `unknown group name 'a'`, `cannot refer to an open group`.
  - `(?P` followed by anything else gives `unknown extension ?P` plus that character, for example `unknown extension ?P>`.
  - `(?:…)`.
  - `(?#…)`: a comment up to the first `)` token. End of input gives `missing ), unterminated comment` at the `(`.
  - `(?=…)`, `(?!…)`, `(?<=…)`, `(?<!…)`. `(?<x` gives `unknown extension ?<x`, and `(?<a>x)` is therefore **invalid**.
  - `(?(cond)yes|no)`:
    - `cond` that is ASCII-decimal is a group number: 0 gives `bad group number`; a number < MAXGROUPS is checked **at the end of the parse** (`invalid group reference N`).
    - Otherwise `cond` is an identifier, which must already exist (`unknown group name`).
    - `yes` and `no` are *sequences*, not alternations. A third `|` gives `conditional backref with more than two branches`.
  - `(?>…)`: an atomic group.
  - Flags `(?aiLmsux)`: see S7.
  - Any other character gives `unknown extension ?c`.
  - A missing `)` gives `missing ), unterminated subpattern` at the `(`.
- S7. Flags (`_parse_flags`, `_parser.py:893-951`). The letters are `i L m s x a u`, plus `t` (3.12).
  - str rejects `L`: `bad inline flags: cannot use 'L' flag with a str pattern`.
  - bytes rejects `u`.
  - More than one of `a u L` gives `bad inline flags: flags 'a', 'u' and 'L' are incompatible`.
  - `(?flags)` with no `:` is **global**. It is legal only while nothing has been parsed yet in the first branch of the top level; verbose whitespace and comments do not count. Otherwise `global flags not at the start of the expression`, at the `(`.
  - Scoped forms: `(?flags:…)`, `(?-flags:…)`, `(?flags-flags:…)`.
  - Errors:
    - `missing -, : or )`;
    - `unknown flag`;
    - `missing flag`;
    - `missing :` (for example `(?-i)`);
    - `bad inline flags: cannot turn off flags 'a', 'u' and 'L'`;
    - `bad inline flags: cannot turn on global flag` (`(?t:…)`);
    - `bad inline flags: flag turned on and off`.
- S8. `^` is AT_BEGINNING; `$` is AT_END; `.` is ANY.
- S9. Alternation `|`: empty branches are legal (`a|`, `(|a)`, `|`). After each top-level `|`, verbose mode is recomputed from the global flags.

**After the parse** (`_parser.py:969-990`, `_compiler.py:149-156,745`):
- P1. A leftover `)` gives `unbalanced parenthesis`, at that `)`.
- P2. Every numeric conditional must name an existing group: `invalid group reference N`, at the recorded position.
- P3. `fix_flags`:
  - str with `a` and `u` gives **ValueError** `ASCII and UNICODE flags are incompatible`;
  - str with neither adds `u`;
  - bytes with `a` and `L` gives ValueError `ASCII and LOCALE flags are incompatible`.
- P4. **R-LB:** every look-behind body must have a fixed width (`getwidth()` lo == hi, `_parser.py:178-227`). Otherwise `look-behind requires fixed-width pattern` (no position). Width rules:
  - literal, class, `.` and category count 1;
  - repeats multiply;
  - branches take the min and max;
  - a group reference takes the referenced group's width;
  - a conditional takes the min and max of its branches (lo = 0 when there is no `no` branch);
  - an unbounded repeat of a non-empty item has infinite width.

  Inside a look-behind, a backreference to a group opened **in the same look-behind** gives `cannot refer to group defined in the same lookbehind subpattern`, and one to a still-open group gives `cannot refer to an open group` (`_parser.py:103-109`).
- P5. `(?t)` together with any repeat gives `internal: unsupported template operator MAX_REPEAT` (or `MIN_REPEAT`, `POSSESSIVE_REPEAT`, whichever comes first in compile order). There is no position.
- P6. Nesting deeper than 484 groups gives **Recursion**.

**Warnings.** FutureWarning texts are those of S4, collected in order. CPython de-duplicates per source location; snp prints each distinct (pattern, message) once (§A.4).

#### A.6.3 Flags model

- Global flags start from `Options`:
  - `ignore_case` gives `i`;
  - `multiline_dotall` gives `m` and `s`.
  - Then `(?…)` at the start is OR-ed in.
- Inside `(?add-del:…)`: `flags = ((add has a|u|L) ? flags & !TYPE : flags) | add & !del` (`_compiler.py:30-34`).
- The **effective flags at each AST node** drive the lowering. snp **never emits** inline flags of its own except `(?s:.)`, `(?m:^)`, `(?m:$)` and `(?i:\N)` for backreferences. `(?x)` and `(?i)` are never emitted.
- "ASCII semantics" is `a` or `L` in str, or **always** in bytes mode.

#### A.6.4 Lowering of atoms (common to both modes)

The emitted text is valid for regex 1.13 **and** fancy-regex 0.19 unless it is marked T2, in which case it goes to fancy only.
- **Literals:**
  - ASCII `[0-9A-Za-z]` are emitted raw.
  - Every other code point or byte is emitted hex-escaped: str as `\x{H…}`, bytes as **`\xHH` (exactly 2 digits)**.
  - Never emit raw whitespace, `#`, `<`, `>`, or an escape that changes meaning between engines: `\<`, `\>`, `\Z`, `\N`, `\h`, `\K`, `\G`, `\R`, `\O`, `\e`, `\k`, `\g`, `\p`. The only exception is the category text in the table below.

| Python AST node | str lowering | bytes lowering | T2? |
|---|---|---|---|
| LITERAL c | `lit(c)`. Surrogate → NEVER = `[^\x{0}-\x{10FFFF}]`. With IGNORECASE → §A.6.6 | `lit(b)`; IGNORECASE → ASCII class | — |
| NOT_LITERAL c (`[^c]`) | `[^…]` built like a class (§A.6.7) | same | — |
| ANY `.` | `.`, or `(?s:.)` with DOTALL | same (regex::bytes `unicode(false)`: any byte except `\n`) | — |
| IN (class) | §A.6.7 | §A.6.7 | — |
| CATEGORY `\d` / `\D` | `\d` / `\D` (Unicode); with `a`: `[0-9]` / `[^0-9]` | `[0-9]` / `[^0-9]` | — |
| CATEGORY `\s` / `\S` | `[\s\x{1C}-\x{1F}]` / `[^\s\x{1C}-\x{1F}]`; with `a`: `[\x{9}-\x{D}\x{20}]` / `[^…]` | `[\x09-\x0D\x20]` / `[^\x09-\x0D\x20]` | — |
| CATEGORY `\w` / `\W` | `[\p{L}\p{N}_]` / `[^\p{L}\p{N}_]`; with `a`: `[0-9A-Za-z_]` / `[^0-9A-Za-z_]` | `[0-9A-Za-z_]` / `[^0-9A-Za-z_]` | — |
| BRANCH | `(?:A|B|…)`, empty branches kept | same | — |
| SUBPATTERN capturing | `(…)`. Names are **dropped**: Python and Rust group numbers coincide because snp emits no other capturing groups. | same | — |
| SUBPATTERN non-capturing / scoped flags | `(?:…)`, inner nodes lowered under the new flags | same | — |
| REPEAT | §A.6.8 | §A.6.8 | possessive: T2 |
| AT `^` / `\A` / `$` / `\Z` / `\b` / `\B` | §A.6.5 | §A.6.5 | maybe |
| GROUPREF `\N`, `(?P=n)` | `(?:\N)`. With IGNORECASE: `(?i:\N)`. The wrapper prevents `\1` followed by a digit literal from reading as `\10`. | same | T2 |
| GROUPREF_EXISTS | `(?(N)yes|no)` or `(?(N)yes)`, **always numeric**. fancy 0.19 mis-evaluates `(?(name)…)`: `(?P<n>a)?(?(n)b|c)` fails on `ab`. | same | T2 |
| ASSERT / ASSERT_NOT | `(?=…)` `(?!…)` `(?<=…)` `(?<!…)` | same | T2 |
| ATOMIC_GROUP | `(?>…)` | same | T2 |
| FAILURE (3.13 `(?!)`) | NEVER | `[^\x00-\xFF]` | — |

#### A.6.5 Anchors: one rule for both modes

**Python semantics:**
- `\A` and `^` without MULTILINE mean the start of the haystack.
- `^` with MULTILINE means the start, or a position after any `\n`. It also matches at the very end after a final `\n`.
- `$` with MULTILINE means before any `\n`, or the end.
- **`$` without MULTILINE means the end, or before a `\n` that is the last character.** `$` on `"\n"` is True; `a$` on `"a\n\n"` is False.
- **`\Z` means the absolute end only.**
- `\b` and `\B` are boundaries by the active `\w`. **Both are False on the empty haystack.**

| Python | Canonical (exact, always correct) | Allowed cheaper form, and its precondition | Tier |
|---|---|---|---|
| `\A` | `\A` | — | 1 |
| `^` (non-M) | `^` | — | 1 |
| `^` (M) | `(?m:^)` | — | 1 |
| `$` (M) | `(?m:$)` | — | 1 |
| **`$` (non-M)** | **`(?=\n?\z)`** (T2) | **(a)** `\z`, if the haystack does not end with `\n`. **(b)** `(?m:$)`, if the haystack contains at most one `\n` and only at its end. This is pygrep by-line (`Haystack::Line`); an exact identity. | fast: 1 |
| **`\Z`** | **`\z`** | — | 1 |
| `\z` | invalid (`bad escape \z`) | — | — |
| `\b` str-Unicode | `(?:(?<=W)(?!W)|(?<!W)(?=W))` with `W=[\p{L}\p{N}_]` (T2) | `\b`, if the haystack is ASCII (`h.is_ascii()`) | fast: 1 |
| `\B` str-Unicode | `(?:(?!\A)|(?!\z))(?:(?<=W)(?=W)|(?<!W)(?!W))` (T2) | `\B`, if the haystack is ASCII **and** non-empty | fast: 1 |
| `\b` / `\B` with `a`, or bytes | the same forms with `W=[0-9A-Za-z_]` | bytes tier 1: `\b`, and `\B` if non-empty (regex::bytes `unicode(false)` is ASCII); str `(?a)`: `\b` / `\B` if ASCII | fast: 1 |

**Consequences.**
- *Config patterns* (`Path`) compile a **fast** tier-1 regex with `$`→`\z` (and `\b`→`\b`), plus an **exact** fancy regex that is **built lazily**.
- At match time: `use_exact = (cond.nl && h.ends_with('\n')) || (cond.ascii && !h.is_ascii()) || (cond.nonempty && h.is_empty())`.
- *Note (snp ADR-098):* the exact fallback of a `\b`/`\B` (or match-span) condition runs behind the §A.6.11 relaxed prefilter too.
- In the corpus, 2,891 tier-1 patterns carry `cond.nl` and 7 carry `cond.ascii`. **No real path ended with `\n`**, so the exact variants were never built in the throughput runs.
- *pygrep by-line* (`Line`) uses form (b). It is exact, needs no dispatch, and matches 04a's probe of `foo$`.
- *pygrep `--multiline`* has M on globally. A non-M `$` arises only from `(?-m:…$)` and follows the config rule, since the haystack is a whole file.
- `^$` deserves a special case. Python `re.search('^$', h)` is `h == "" || h == "\n"`. snp must use exactly this, both for the default `exclude` and wherever else it appears. pre-commit's own `exclude == '^$'` short-cut in `check_useless_excludes` is a **string** comparison and stays one.

#### A.6.6 IGNORECASE lowering

- **str, Unicode semantics** (no `a`/`L`):
  - A literal `c` becomes the class `fold(c)`.
  - A class becomes `fold(members)`. Categories inside it are **not** folded.
  - `fold(S)` is the Unicode simple case folding closure of `S`; use `regex_syntax::hir::ClassUnicode::case_fold_simple()` over the literal and range items only.
  - Then, if the result intersects `{U+0049, U+0069, U+0130, U+0131}`, add all four (§A.5.3).
  - Emit the result as an explicit class. **Never emit `(?i)` around anything except a backreference.**
  - Exhaustively verified against Python at the literal level (0 differences besides the Turkish-i rule at UCD 16). Fuzz-verified at the class level (0 differences in 18,342 patterns).
  - Python's "no member is cased ⇒ no folding" rule (`hascased`) needs no special code: for such sets `fold(S) = S`.
- **str with `a` or `L`, and bytes:** for each ASCII letter in a literal or range, add its other case. Non-ASCII members are left unfolded. `(?ai)K` matches `k`/`K` only, not the Kelvin sign U+212A; `(?i)` in bytes leaves `\xC9` and `\xE9` unrelated.
- **NOT_LITERAL** and negated classes: fold first, then negate. `(?i)[^i]` matches neither `ı` nor `İ`.
- **Backreferences:** `(?i:\N)` (T2). See D1.

#### A.6.7 Classes

1. Collect the items: literal code points, ranges, and categories (in the effective mode).
2. In str mode, clip ranges to exclude D800–DFFF and drop surrogate literals.
3. Apply IGNORECASE (§A.6.6).
4. Emit `[` + (`^` if negated) + items + `]`:
   - each literal/range is hex-escaped per §A.6.4, so `-`, `]`, `[`, `^`, `&`, `~`, `\` can never form Rust set syntax (`&&`, `--`, `~~`, nested `[`);
   - categories are emitted as the class *contents* from the §A.6.4 table: `\d`, `\s\x{1C}-\x{1F}`, `\p{L}\p{N}_`, `0-9A-Za-z_`. **Negated categories** are emitted as nested classes: `[^\s\x{1C}-\x{1F}]`, `[^\p{L}\p{N}_]`, `[^0-9]`. Both engines accept nested classes; this was verified for `[a[^\s]]`.
5. An empty positive class becomes NEVER; an empty negated class becomes `(?s:.)`.

#### A.6.8 Quantifiers

| Python | Emit |
|---|---|
| `X*` `X+` `X?` `X{m}` `X{m,}` `X{m,n}` (n = 4294967295 means unbounded) | `(?:X)*` `(?:X)+` `(?:X)?` `(?:X){m}` `(?:X){m,}` `(?:X){m,n}`, always wrapping the target in `(?:…)` |
| lazy (`X*?` …) | append `?` |
| possessive (`X*+`, `X{m,n}+`) | append `+` (**T2 only**). Never send these to `regex`: there `a*+` / `a{2}+` / `a++` are nested repeats with different semantics. |
| **X with max width 0** (look-around, anchors inside a group, `(?:)`, `()`, or nestings of these, e.g. `(?:(?<!a){2})+`) | **m ≥ 1**: `(?:X)`. **m = 0**: `(?:X|)` greedy or possessive, `(?:|X)` lazy. fancy rejects any quantifier on a look-around ("Target of repeat operator is invalid"); for zero-width items this rewrite is semantically identical. |
| counts > 1000 | allowed. Tier 1 is built with `size_limit(64 MiB)` and `dfa_size_limit(64 MiB)`. `a{1000000}` compiles; `a{4294967294}` does not and gives **Unsupported** (§A.6.13 D5). Corpus maximum: 111. |

#### A.6.9 Tier selection

- **T2** if the AST contains any of: look-around, a backreference, a conditional, an atomic group, or a possessive quantifier.
- Otherwise **T1**: a `fast` text, with an optional `exact` text and the condition bits from §A.6.5.
- T2 patterns get `exact` (fancy) plus `relaxed` (the T1 prefilter, §A.6.11).

#### A.6.10 Engine construction, and the bytes lanes

| Use | Builder |
|---|---|
| T1 str | `regex::RegexBuilder::new(fast).size_limit(64<<20).dfa_size_limit(64<<20).nest_limit(2048)` |
| T1 bytes | `regex::bytes::RegexBuilder::new(fast).unicode(false)` + the same limits. Byte literals are `\xHH`, so `\xC3(?:\xA9)+` quantifies **the last byte**, as Python bytes does. |
| T2 / exact, str | `fancy_regex::RegexBuilder::new(exact).backtrack_limit(100_000_000).delegate_size_limit(64<<20).delegate_dfa_size_limit(64<<20)` (default `BytesMode::Unicode`). The emitted nesting depth must be < 64 (fancy `MAX_RECURSION`); otherwise give **Unsupported** (D6). |
| T2 / exact, bytes ("**Latin-1 lane**") | the same fancy builder (Unicode mode) over a haystack transcoded `String` where each byte `b` becomes `char::from(b)` (U+0000–U+00FF; a bijection). The pattern uses `\xHH`, which fancy reads as U+00HH. Every class is explicit ASCII (§A.6.4), so the Unicode mode never widens `\w`/`\s`/`\b`. Match offsets are char indices; convert them back to byte offsets by counting chars (each is one byte). Verified with `\x{C3}\x{A9}+` vs `a\xc3\xa9\xa9\xa9x` → (0,5), and `[\x{80}-\x{FF}]` vs `\xff`. |

Two things **not** to do:
- **Do not** use fancy `BytesMode::Ascii` for patterns. `\xFF` would mean UTF-8 `C3 BF`, and `[\x80-\xFF]` fails to compile.
- **Do not** use fancy's `seek(true)`. It is experimental and documented as possibly skipping matches. Use §A.6.11 instead.

#### A.6.11 Relaxed prefilter for tier 2 (mandatory)

Build `relaxed` from the same AST with these substitutions:
- look-around → empty;
- backreference → `(?s:.*)`;
- conditional `(?(N)Y|Z)` → `(?:Y|Z)`;
- atomic → `(?:…)`;
- possessive → greedy;
- non-M `$` → `(?m:$)`;
- `\b` and `\B` → empty.

Everything else lowers as in tier 1. Each substitution only **adds** matches, so `L(P) ⊆ L(relaxed)` under search semantics. Hence `!relaxed.is_match(h) ⇒ !P.is_match(h)`.

At match time: `if let Some(r) = relaxed { if !r.is_match(h) { return Ok(false) } } fancy.is_match(h)`.

Skip the prefilter only if `relaxed` matches the empty string at every position (it has no anchors or look-arounds left, and `relaxed.is_match("")` is true). Example: `(?!foo)` relaxes to the empty string.

Gains: 6.3× on the corpus' 42 T2 config patterns; 40× and 1,200× on pygrep look-around patterns (§A.2.4).

*Note (snp ADR-098):* snp also puts this prefilter (the fullmatch engines wrap it in `\A(?:…)\z`) in front of the tier-1 exact fallback of a `\b`/`\B` or match-span condition, built lazily with a 256 KiB lazy-DFA cache.

#### A.6.12 Matching API, caching and fast paths

- `files`/`exclude` use `search`. The haystack is the git-relative path with `/` separators. On Windows, convert `\` to `/` **before** matching (`pre-commit/pre_commit/commands/run.py:123-124`).
- Builtin hooks that call `re.match` (`no-commit-to-branch --pattern`, 09) use `match_prefix`, lowered as `\A(?:P)`. Those that call `re.fullmatch` (`name-tests-test` E2) use `\A(?:P)\z`. The translator's `$` rule applies inside P unchanged.
- **Cache** compiled patterns per `(source, Mode, Options)` in a `HashMap<Key, Arc<PyRegex>>` that lives for the run. Compile at config load (validation), and reuse the result in the meta hooks and in `run`.
- **Trivial patterns.** `''` always matches. `'^$'` is `h.is_empty() || h == "\n"`. These two are the defaults and cover most hooks; they must not reach an engine.
- **Per-path evaluation of a hook set (optional).** Put the distinct T1 `fast` texts of a run into one `regex::RegexSet`. For each path, take the set bits once, then decide every hook from the bits plus individual T2 checks. Measured gain: 5 % on the 40 largest configs. Keep it optional and behind a benchmark gate.
- **Errors at match time.**
  - If `fancy` exceeds its backtrack limit, return `MatchError::Backtrack`. For `files`/`exclude` this aborts the run with `An error has occurred: snp: regular expression {pattern!r} exceeded the backtracking limit on {path!r}` and exit 1. CPython would just run for a long time; this is Tier B and is recorded in `known-divergences.yaml`.
  - For pygrep, the hook fails with that message as output and exit 1.

#### A.6.13 Residual divergences (complete list; corpus frequency in the last column)

| # | Construct | Python | snp | Frequency |
|---|---|---|---|---|
| D1 | IGNORECASE + backreference (`(?i)(k)\1` …) | per character `lower(a) == lower(b)`, without `_EXTRA_CASES` | fancy: simple-fold equality **with equal UTF-8 byte length**. Differs for `K`/`k` (3 vs 1 bytes), `ſ`/`s` and `ς`/`σ` (Python False, fancy True), `İ`. In bytes / `(?a)` mode fancy also folds U+00C0–U+00FE in the Latin-1 lane. | 40 / 18,382 fuzz patterns; corpus 0 |
| D2 | Characters unassigned in UCD 15.0 (Python 3.12) but assigned in UCD 16 | 3.12: not `\w`, no case pairs, `\N{…}` unknown | treated as in 3.14 | — |
| D3 | `\N{…}` with the feature disabled | accepted | `Unsupported` | 0 |
| D4 | `(?L)` (bytes) under a single-byte non-UTF-8 locale | locale `isalnum` | ASCII | 0 |
| D5 | a repeat count so large that tier 1 exceeds 64 MiB (≈ > 10⁷ simple atoms) | compiles | `Unsupported`, reported as the validation error `'…' is not a valid python regex` plus an snp hint on stderr | 0 |
| D6 | a T2 pattern (look-around, backref, …), or an exact fallback, with emitted nesting ≥ 64 | compiles | `Unsupported` (fancy `MAX_RECURSION`) | 0 (corpus maximum depth 8) |
| D7 | catastrophic backtracking beyond 10⁸ steps in T2 | runs (minutes or hours) | `MatchError::Backtrack` | 0 |
| D8 | exponential backtracking in **T1** (`^(.+/)*x$` on deep paths) | hangs | linear time, same answer | real (2 corpus patterns) |
| D9 | OverflowError / ValueError / RecursionError in `files`/`exclude` | exit 3 crash | validation error, exit 1 | 0 |
| D10 | FutureWarning text | `…/cfgv.py:386: FutureWarning: …` | snp `[WARNING] …` line | 1 |
| D11 | pygrep: an invalid entry under N partitions | N tracebacks | 1 line | — |

### A.7 Changes made to other specs, and contradictions resolved

| Spec | Change (edited in place; each edit is marked "→ 01 Addendum") |
|---|---|
| 04a §12.3 | A superseding note was inserted under the heading. The tier idea, the argparse emulation, the matching procedures (§12.4) and the test vectors stay. Corrected rules: tier 2 runs on the **Latin-1 lane**, not `BytesMode::Ascii`; IGNORECASE uses explicit ASCII classes, not `(?i:…)`; `\z` is **rejected**, because 3.12 is the target; the `\b`/`\B` forms in tier 2 are look-arounds; the relaxed prefilter is mandatory; the backtrack limit is 10⁸. |
| 07 D4 + §3.2 "Recommendations" + §6 crate comments + §7 item 19 | The single-engine decision is **withdrawn**. D4 now reads "two-tier: regex 1.13.1 (tier 1, bytes, prefilters) + fancy-regex 0.19.2 (tier 2) behind snp-pyre; see 01 Addendum". |
| 10 Q6 | Decided: two-tier (see this addendum). Also, 10 §7.5.3 suggests `$`→`(?=\n?\z)` as the only form. That remains the canonical form; §A.6.5 adds the proven-equivalent fast forms. |
| 01 §16 table | Add the rows for OverflowError, ValueError and RecursionError (§A.4). The addendum cannot edit §16 in place (append-only rule), so these rows are normative from here. |

**Contradiction "Regex engine"** (01 §13/§18.1 + 04a §12.3: two tiers; 07 D4: fancy only; 10 Q6: undecided).
- **Resolved: two tiers.** The deciding facts are measured and listed in §A.2.5 and §A.2.6:
  1. pygrep needs `regex::bytes` for bytes ≥ 0x80.
  2. The tier-2 prefilter must be a `regex`.
  3. Tier 1 is 29 % faster than routing it through fancy.
  4. fancy cannot nest 64 or more groups.
- 07 D4's own numbers (2.9 vs 3.3 ms, "≈ 15 %") were reproduced, and on the full corpus the gap is larger (7.27 vs 9.38 ms).
- 07 D4's argument that "regex rejects look-around, `{,n}` and `a{`" no longer applies: the snp parser lowers those (literal `{` → `\x{7B}`, `{,n}` → `{0,n}`, look-around → tier 2).

**Contradiction "In-process emulation granularity"**, regex part only.
- **Resolved:** compile once, report an invalid pattern once, exit 1 (§A.4.5).
- Evidence: pre-commit prints one traceback **per partition** (19 for 40,000 files). This cannot be byte-matched (interpreter paths) and is a Tier B difference.
- Everything else about partitions stays as 09 decided.

The other contradictions in the gap list (stdin, YAML text, async runtime, toolchain, hazmat, `PRE_COMMIT_COLOR`, argparse prefix, env-dir naming, install state, store, crash log, `which`, CPU count, fsmonitor, stash restore, pre-push parsing) do not involve regexes. This addendum does not touch them.

### A.8 Test plan (all vectors generated by CPython 3.12 at authoring time; CI needs no Python)

1. **Acceptance golden file**:
   - all 509 edge cases (`scripts/edge_cases.py`);
   - all 4,858 corpus patterns;
   - 20,000 fuzz patterns.

   Each vector records `{pattern, mode, ok | (kind, msg, pos)}`. For `str` patterns snp must match the `ok` flag. For `bytes` patterns it must also match `kind`, `msg` and `pos`, because pygrep prints them. Also include the 3.10–3.14 table rows of §A.3 as documentation-only vectors.
2. **Match golden file**: (pattern, path-set id) → (count, FNV-1a-64 of the matching indices) for the corpus and the fuzz set. Commit the path sets `tricky` (132), `gen` (51,244) and `fuzzhay` (4,329). The 3.87 M repo-tree paths stay out of git: regenerate them with `scripts/fetch_trees.sh` in a nightly job.
3. **Exhaustive tables**: Python `\w`/`\d`/`\s` per code point and `(?i)` literal sets (`results/pyclasses.py312.json`, `results/fold312.json`). Assert that snp's emitted classes are equal, with the UCD 15→16 delta allow-listed (D2).
4. **pre-commit end-to-end** (`scripts/pc_probe.sh`): the §A.4 rows, nesting at 484 vs 485, the FutureWarning case, and pygrep with 40,000 files.
5. **Property tests (proptest)**:
   - `compile` never panics;
   - parsing is linear in pattern length;
   - `relaxed ⊇ exact` on random haystacks;
   - the fast and exact variants agree on every haystack that satisfies the condition bits.
6. **Fuzz target `py_regex_compile`** (cargo-fuzz): input bytes → (mode, pattern) → compile, plus search on 8 fixed haystacks. Assertions: no panic, no allocation over 256 MiB, each case done in under 100 ms.
7. **Performance gates** (criterion): over `paths/bench100k.paths`, each must hold:
   - tier 1 ≤ 9 ms per pattern per 100k paths (measured 7.27);
   - tier 2 with prefilter ≤ 60 ms (measured 36);
   - the config-level benchmark of §A.2.4 ≤ 350 ms per config (measured 234–301).

### A.9 Implementation notes for snp (regex layer)

- **Crate `snp-pyre`** modules:
  - `tok` (str: `Vec<char>` with indices; bytes: `&[u8]`);
  - `parse` (AST in an arena: `Vec<Node>`, `NodeId(u32)`);
  - `check` (widths, group references, flags);
  - `lower` (one `fmt::Write` emitter, parameterised by `Variant::{Fast, Exact, Relaxed}`);
  - `fold` (uses `regex_syntax::hir::ClassUnicode::case_fold_simple`);
  - `names` (feature `py-re-names`, `unicode_names2 = "=4.0.0"`, plus the generated alias table);
  - `engine`.

  `#![forbid(unsafe_code)]`. Deny `clippy::unwrap_used`, `indexing_slicing` and `panic`.
- **Dependencies:**
  - `regex = "1.13.1"` (default features: `std`, `perf`, `unicode`);
  - `regex-syntax = "0.8.11"` (case folding and class sets, already a transitive dependency);
  - `fancy-regex = "0.19.2"`. Default features were verified. `variable-lookbehinds` is *not* needed for Python-legal look-behinds: char-width constants such as `(?<=é|e)` and `(?<=\x{1F600})` work without it. Drop it only after the parity suite passes.
  - `unicode-ident` (for `isidentifier`).
- **Never call `regex::Regex::new` or `fancy_regex::Regex::new` on user text.** That includes `cfgv-rs`'s `CheckFn::Regex` (`cfgv-rs/src/check.rs:116-118`): replace it with an `snp-pyre`-backed custom check (07 §7 item 28 "cfgv-rs 0.2.0 gaps" (a)).
- **Parser:**
  - It must be iterative, or recursion with a hard cap (484) and a large-enough stack.
  - Track positions in *code points* for str patterns. Python positions are character indices, so convert to them when formatting errors.
  - Precompute `getwidth` bottom-up with saturating `u64` (Python's `MAXWIDTH = 1<<64` ⇒ use `u128` or a saturating flag).
- **Emission:**
  - Pre-size the output `String` to about 2× the source length.
  - Always hex-escape non-alphanumerics. Do not "pretty-print" literals: a single escaping rule is what made raw-engine differences disappear.
  - Never let user text reach an engine unescaped (no `(?x)`, no raw `#`, no whitespace).
- **Engine type:**

  ```rust
  enum Compiled {
      Trivial(TrivialKind),                                     // '' | '^$'
      T1 { fast: regex::Regex, cond: Cond, exact_src: Option<Box<str>>, exact: OnceLock<Result<fancy_regex::Regex, Arc<str>>> },
      T2 { pre: Option<regex::Regex>, re: fancy_regex::Regex },
  }
  ```

  The byte variants use `regex::bytes::Regex` and the Latin-1 lane. All are `Send + Sync`; share them via `Arc` across the run's worker threads. `regex` keeps a per-thread cache pool internally.
- **Memory:** about 25 KiB per tier-1 pattern. A typical config (median 11 hooks, mostly `''`/`'^$'`) needs well under 1 MiB. The 5 largest corpus patterns (up to 39,964 characters) take 67 ms and 16 MiB to compile in total, the same order as CPython's 62 ms. Build exact variants **lazily**: eager building doubles RSS (42 vs 25 KiB per pattern).
- **Haystack checks:**
  - `h.ends_with('\n')` is O(1).
  - `h.is_ascii()` is a SIMD word scan. Compute it only for patterns with `cond.ascii`; that was 7 of 4,646.
- **Windows:** normalise `\`→`/` first. macOS: no Unicode normalisation.
- **Non-UTF-8 paths:** these are out of scope here. pre-commit crashes on them. If 02/07 decide to support them, run str patterns on `String::from_utf8_lossy`, or reject.
- **Logging:** under `-v`, log the tier, the emitted text and the condition bits per pattern. This is invaluable when users report differences.
- **Pitfalls checklist:**
  1. `\Z` → `\z`. Never pass `\Z` to fancy, where it means "ignore trailing newlines".
  2. `$` (non-M) ≠ Rust `$`. Use the §A.6.5 table.
  3. Literal `{` is common in cookiecutter repos.
  4. `(?x)` patterns in YAML block scalars are 12 % of the corpus. Python keeps whitespace inside classes; Rust `(?x)` does not.
  5. `[[` is a literal `[`, not a nested class.
  6. `\b` inside a class is backspace.
  7. `[\1]` is octal 1.
  8. `\12` is a backreference only if group 12 exists; otherwise it is an error. `\123` is octal.
  9. Possessive and atomic constructs go to fancy only.
  10. A quantified look-around is legal in Python; rewrite it per §A.6.8.
  11. IGNORECASE: explicit classes plus the Turkish-i rule. Never `(?i)` on categories.
  12. Bytes mode: `\xHH` only; never `\x{…}`, never `BytesMode::Ascii`.
  13. Raise fancy `backtrack_limit` to 10⁸ and always use the prefilter.
  14. fancy nesting must stay below 64.
  15. Report OverflowError, ValueError and RecursionError as validation errors (exit 1), not crashes.

### A.10 Open questions

1. **Python dialect.** Should snp follow 3.12 (decided) or the newest Python, where `\z` became legal in 3.14? Proposal: stay on 3.12 until pre-commit drops 3.13. Add an opt-in `SNP_PY_RE=3.14`, which only adds `\z` acceptance.
2. **Crash-to-validation-error policy** for OverflowError, ValueError and RecursionError. This follows 01 §16 Q3. If the project later prefers exit-3 parity, `PyReError::{Overflow, Value, Recursion}` map one-to-one to `An unexpected error has occurred: OverflowError: …` and so on.
3. **D1 (case-insensitive backreferences).** Accept the divergence (corpus 0), or implement Python's per-character `lower()` comparison by contributing a `casei_compare` hook to fancy-regex upstream?
4. **`\N{…}` support.** Is the extra 1.3 MB of binary acceptable as a default-on feature? Corpus usage is 0.
5. **FutureWarning rendering.** Use `[WARNING] …` on stderr (proposed), stay silent, or emulate the `file:line: FutureWarning:` form with a fixed fake path?
6. **Backtrack-limit failure on `files`/`exclude`.** Abort the run (proposed), or treat the path as "not matched" with a warning, as prek does silently with `unwrap_or(false)` (rejected: silent)?
7. **RegexSet across hooks.** Is the measured 5 % gain worth the complexity, or should it wait for the end-to-end benchmark (10 B4)?

### A.11 Reproduction

All paths below are relative to `scratchpad/rxgap/`.

- **Corpus:**
  - `scripts/fetch_lists.sh` (gh search API);
  - `scripts/fetch_raw.sh` (raw.githubusercontent.com);
  - `scripts/fetch_trees.sh` (git trees API);
  - `scripts/extract_corpus.py`, run with `venv-pc462/bin/python -I`. It parses YAML with CSafeLoader; Python test sources go through `ast` and are never executed.
- **Oracles:**
  - `scripts/pyprobe.py` (acceptance, per Python version);
  - `scripts/pymatch.py`, `scripts/pymatch_one.sh` (matching);
  - `scripts/pymatchb.py` (pygrep);
  - `scripts/pyclasses.py`, `scripts/cmp_classes.py`, `scripts/cmp_fold.py` (Unicode).
- **Translator prototype:** `scripts/translate.py`. It lowers CPython 3.12's own AST using exactly the §A.6 rules, so it is a reference for the lowering half only.
- **Rust probes:**
  - `rxcorpus/` (`accept`, `match`, `matchb`, `bench`, `configbench`, `mem`, `sem`, `classes`);
  - `rxbytes/` (bytes and Latin-1 lane, prefilter);
  - `rxnames/` (`unicode_names2`);
  - `rxlb/` (look-behind features).
- **Results:** `results/*.jsonl`, `results/*.json`. Key files:
  - `uniq.py312.jsonl`, `uniq.rust_raw.jsonl` (acceptance);
  - `pymatch.jsonl` and `rustmatch.jsonl` (str parity);
  - `pymatchb.jsonl` and `rustmatchb.jsonl` (pygrep parity);
  - `fuzz_py.jsonl` and `fuzz_rs.jsonl`;
  - `edge.py3{10..14}.jsonl`.
