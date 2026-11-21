# JavaScript / TypeScript project

A config for a Node.js project with TypeScript:

* [prettier](https://github.com/rbubley/mirrors-prettier) as a remote hook: snp builds a Node environment for it, as
  pre-commit does, so its version is pinned by the `rev`;
* ESLint and `tsc --noEmit` as `repo: local` hooks that run the project's own `node_modules` through
  `npx --no-install`, so the versions and plugins are those of `package.json`;
* generic checks from [pre-commit-hooks](https://github.com/pre-commit/pre-commit-hooks) v6.0.0, which snp runs
  natively. `check-json` skips `tsconfig*.json`, which allow comments.

The file uses no snp extension, so teammates who still run pre-commit 4.7.0 can share it unchanged.

## Run it

```sh
cp examples/javascript-typescript/.pre-commit-config.yaml /path/to/your/repo/
cd /path/to/your/repo
npm ci                        # the local hooks need node_modules
snp install --install-hooks
snp run --all-files
```

The prettier hook needs `node` and `npm` on `PATH` the first time snp installs it.
