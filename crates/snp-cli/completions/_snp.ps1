
using namespace System.Management.Automation
using namespace System.Management.Automation.Language

Register-ArgumentCompleter -Native -CommandName 'snp' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandElements = $commandAst.CommandElements
    $command = @(
        'snp'
        for ($i = 1; $i -lt $commandElements.Count; $i++) {
            $element = $commandElements[$i]
            if ($element -isnot [StringConstantExpressionAst] -or
                $element.StringConstantType -ne [StringConstantType]::BareWord -or
                $element.Value.StartsWith('-') -or
                $element.Value -eq $wordToComplete) {
                break
        }
        $element.Value
    }) -join ';'

    $completions = @(switch ($command) {
        'snp' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('-V', '-V ', [CompletionResultType]::ParameterName, 'show program''s version number and exit')
            [CompletionResult]::new('--version', '--version', [CompletionResultType]::ParameterName, 'show program''s version number and exit')
            [CompletionResult]::new('autoupdate', 'autoupdate', [CompletionResultType]::ParameterValue, 'Auto-update snp config to the latest repos'' versions.')
            [CompletionResult]::new('cache', 'cache', [CompletionResultType]::ParameterValue, 'Inspect or clean snp''s cache.')
            [CompletionResult]::new('clean', 'clean', [CompletionResultType]::ParameterValue, 'Clean out snp files.')
            [CompletionResult]::new('completions', 'completions', [CompletionResultType]::ParameterValue, 'Print a shell completion script.')
            [CompletionResult]::new('config', 'config', [CompletionResultType]::ParameterValue, 'Print the merged configuration or its JSON schema.')
            [CompletionResult]::new('doctor', 'doctor', [CompletionResultType]::ParameterValue, 'Check snp, the hook scripts and the cache for problems.')
            [CompletionResult]::new('gc', 'gc', [CompletionResultType]::ParameterValue, 'Clean unused cached repos.')
            [CompletionResult]::new('hazmat', 'hazmat', [CompletionResultType]::ParameterValue, 'Composable tools for rare use in hook `entry`.')
            [CompletionResult]::new('init-templatedir', 'init-templatedir', [CompletionResultType]::ParameterValue, 'Install hook script in a directory intended for use with `git config init.templateDir`.')
            [CompletionResult]::new('install', 'install', [CompletionResultType]::ParameterValue, 'Install the snp script.')
            [CompletionResult]::new('install-hooks', 'install-hooks', [CompletionResultType]::ParameterValue, 'Install hook environments for all environments in the config file.  You may find `snp install --install-hooks` more useful.')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'List the hooks of the configuration.')
            [CompletionResult]::new('migrate', 'migrate', [CompletionResultType]::ParameterValue, 'Convert lefthook configurations or switch pre-commit-hooks to and from `repo: builtin`.')
            [CompletionResult]::new('migrate-config', 'migrate-config', [CompletionResultType]::ParameterValue, 'Migrate list configuration to new map configuration.')
            [CompletionResult]::new('run', 'run', [CompletionResultType]::ParameterValue, 'Run hooks.')
            [CompletionResult]::new('sample-config', 'sample-config', [CompletionResultType]::ParameterValue, 'Produce a sample .pre-commit-config.yaml file')
            [CompletionResult]::new('self', 'self', [CompletionResultType]::ParameterValue, 'Manage the snp installation.')
            [CompletionResult]::new('try-repo', 'try-repo', [CompletionResultType]::ParameterValue, 'Try the hooks in a repository, useful for developing new hooks.')
            [CompletionResult]::new('uninstall', 'uninstall', [CompletionResultType]::ParameterValue, 'Uninstall the snp script.')
            [CompletionResult]::new('validate-config', 'validate-config', [CompletionResultType]::ParameterValue, 'Validate .pre-commit-config.yaml files')
            [CompletionResult]::new('validate-manifest', 'validate-manifest', [CompletionResultType]::ParameterValue, 'Validate .pre-commit-hooks.yaml files')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Show help for a specific command.')
            [CompletionResult]::new('hook-impl', 'hook-impl', [CompletionResultType]::ParameterValue, 'hook-impl')
            break
        }
        'snp;autoupdate' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--repo', '--repo', [CompletionResultType]::ParameterName, 'Only update this repository -- may be specified multiple times.')
            [CompletionResult]::new('-j', '-j', [CompletionResultType]::ParameterName, 'Number of threads to use. (default 1).')
            [CompletionResult]::new('--jobs', '--jobs', [CompletionResultType]::ParameterName, 'Number of threads to use. (default 1).')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--bleeding-edge', '--bleeding-edge', [CompletionResultType]::ParameterName, 'Update to the bleeding edge of `HEAD` instead of the latest tagged version (the default behavior).')
            [CompletionResult]::new('--freeze', '--freeze', [CompletionResultType]::ParameterName, 'Store "frozen" hashes in `rev` instead of tag names')
            break
        }
        'snp;cache' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('dir', 'dir', [CompletionResultType]::ParameterValue, 'Print the cache directory.')
            [CompletionResult]::new('size', 'size', [CompletionResultType]::ParameterValue, 'Print the size of the cache.')
            [CompletionResult]::new('gc', 'gc', [CompletionResultType]::ParameterValue, 'Remove unused repositories and environments.')
            [CompletionResult]::new('clean', 'clean', [CompletionResultType]::ParameterValue, 'Remove the whole cache.')
            break
        }
        'snp;cache;dir' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;cache;size' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;cache;gc' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;cache;clean' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;clean' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;completions' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;config' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('dump', 'dump', [CompletionResultType]::ParameterValue, 'Print the merged configuration, local overrides included.')
            [CompletionResult]::new('schema', 'schema', [CompletionResultType]::ParameterValue, 'Print the JSON schema of .pre-commit-config.yaml.')
            break
        }
        'snp;config;dump' {
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'Print JSON instead of YAML.')
            [CompletionResult]::new('--no-local-config', '--no-local-config', [CompletionResultType]::ParameterName, 'Ignore the local override file (.pre-commit-config.local.yaml).')
            break
        }
        'snp;config;schema' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;doctor' {
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--fix', '--fix', [CompletionResultType]::ParameterName, 'Repair this repository''s hook scripts, then check again.')
            [CompletionResult]::new('--fast-path', '--fast-path', [CompletionResultType]::ParameterName, 'Explain the native fast-path decision of every hook.')
            [CompletionResult]::new('--stash', '--stash', [CompletionResultType]::ParameterName, 'Check orphaned stashes against the worktree; with --fix, dismiss their notices.')
            break
        }
        'snp;gc' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;hazmat' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('cd', 'cd', [CompletionResultType]::ParameterValue, 'cd to a subdir and run the command')
            [CompletionResult]::new('ignore-exit-code', 'ignore-exit-code', [CompletionResultType]::ParameterValue, 'run the command but ignore the exit code')
            [CompletionResult]::new('n1', 'n1', [CompletionResultType]::ParameterValue, 'run the command once per filename')
            break
        }
        'snp;hazmat;cd' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;hazmat;ignore-exit-code' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;hazmat;n1' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;init-templatedir' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('-t', '-t', [CompletionResultType]::ParameterName, 't')
            [CompletionResult]::new('--hook-type', '--hook-type', [CompletionResultType]::ParameterName, 'hook-type')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--no-allow-missing-config', '--no-allow-missing-config', [CompletionResultType]::ParameterName, 'Assume cloned repos should have a `pre-commit` config.')
            break
        }
        'snp;install' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('-t', '-t', [CompletionResultType]::ParameterName, 't')
            [CompletionResult]::new('--hook-type', '--hook-type', [CompletionResultType]::ParameterName, 'hook-type')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('-f', '-f', [CompletionResultType]::ParameterName, 'Overwrite existing hooks / remove migration mode.')
            [CompletionResult]::new('--overwrite', '--overwrite', [CompletionResultType]::ParameterName, 'Overwrite existing hooks / remove migration mode.')
            [CompletionResult]::new('--install-hooks', '--install-hooks', [CompletionResultType]::ParameterName, 'Whether to install hook environments for all environments in the config file.')
            [CompletionResult]::new('--allow-missing-config', '--allow-missing-config', [CompletionResultType]::ParameterName, 'Whether to allow a missing `snp` configuration file or exit with a failure code.')
            [CompletionResult]::new('--git-config-hooks', '--git-config-hooks', [CompletionResultType]::ParameterName, 'Install the hooks as git config entries (`hook.<name>.command`, git 2.54+) instead of hook scripts.')
            break
        }
        'snp;install-hooks' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--with-fallback', '--with-fallback', [CompletionResultType]::ParameterName, 'Also install the Python environments that hooks run natively by snp would fall back to (for offline use with SNP_NO_FAST_PATH).')
            break
        }
        'snp;list' {
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--hook-stage', '--hook-stage', [CompletionResultType]::ParameterName, 'Only list the hooks of this stage. One of commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg, manual')
            [CompletionResult]::new('--group', '--group', [CompletionResultType]::ParameterName, 'Run only hooks in this group; repeat to select hooks in any of the groups. `@ungrouped` selects hooks without groups.')
            [CompletionResult]::new('--require-group', '--require-group', [CompletionResultType]::ParameterName, 'Run only hooks in this group; repeat to require every group.')
            [CompletionResult]::new('--no-group', '--no-group', [CompletionResultType]::ParameterName, 'Skip hooks in this group (repeatable; wins over --group and --require-group).')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'Print JSON instead of text.')
            break
        }
        'snp;migrate' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('lefthook', 'lefthook', [CompletionResultType]::ParameterValue, 'Convert a lefthook configuration.')
            [CompletionResult]::new('builtin', 'builtin', [CompletionResultType]::ParameterValue, 'Switch pre-commit-hooks repos to `repo: builtin`.')
            [CompletionResult]::new('upstream', 'upstream', [CompletionResultType]::ParameterValue, 'Switch `repo: builtin` back to pre-commit-hooks.')
            break
        }
        'snp;migrate;lefthook' {
            [CompletionResult]::new('--root', '--root', [CompletionResultType]::ParameterName, 'Repository root (default: the current git repository).')
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'lefthook configuration to convert (default: lefthook''s lookup).')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'lefthook configuration to convert (default: lefthook''s lookup).')
            [CompletionResult]::new('-o', '-o', [CompletionResultType]::ParameterName, 'Where to write the result (default: .pre-commit-config.yaml; `-` for stdout).')
            [CompletionResult]::new('--output', '--output', [CompletionResultType]::ParameterName, 'Where to write the result (default: .pre-commit-config.yaml; `-` for stdout).')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--portable', '--portable', [CompletionResultType]::ParameterName, 'Emit only keys pre-commit 4.x understands.')
            [CompletionResult]::new('--local', '--local', [CompletionResultType]::ParameterName, 'Also convert lefthook-local.yml into .pre-commit-config.local.yaml.')
            [CompletionResult]::new('--strict', '--strict', [CompletionResultType]::ParameterName, 'Fail (exit 2) when a feature cannot be converted faithfully.')
            [CompletionResult]::new('--install', '--install', [CompletionResultType]::ParameterName, 'Afterwards remove lefthook''s hooks and install snp''s.')
            [CompletionResult]::new('-f', '-f', [CompletionResultType]::ParameterName, 'Overwrite an existing output file.')
            [CompletionResult]::new('--force', '--force', [CompletionResultType]::ParameterName, 'Overwrite an existing output file.')
            break
        }
        'snp;migrate;builtin' {
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;migrate;upstream' {
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;migrate-config' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;run' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--files', '--files', [CompletionResultType]::ParameterName, 'Specific filenames to run hooks on.')
            [CompletionResult]::new('--stdin-files', '--stdin-files', [CompletionResultType]::ParameterName, 'Run on the file names read from stdin: one per line (`lines`, the default) or NUL-separated (`nul`).')
            [CompletionResult]::new('--hook-stage', '--hook-stage', [CompletionResultType]::ParameterName, 'The stage during which the hook is fired. One of commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg, manual')
            [CompletionResult]::new('--remote-branch', '--remote-branch', [CompletionResultType]::ParameterName, 'Remote branch ref used by `git push`.')
            [CompletionResult]::new('--local-branch', '--local-branch', [CompletionResultType]::ParameterName, 'Local branch ref used by `git push`.')
            [CompletionResult]::new('-s', '-s', [CompletionResultType]::ParameterName, '(for usage with `--to-ref`) -- this option represents the original ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch you are pushing to. For `post-checkout` hooks, this represents the branch that was previously checked out.')
            [CompletionResult]::new('--from-ref', '--from-ref', [CompletionResultType]::ParameterName, '(for usage with `--to-ref`) -- this option represents the original ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch you are pushing to. For `post-checkout` hooks, this represents the branch that was previously checked out.')
            [CompletionResult]::new('--source', '--source', [CompletionResultType]::ParameterName, '(for usage with `--to-ref`) -- this option represents the original ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch you are pushing to. For `post-checkout` hooks, this represents the branch that was previously checked out.')
            [CompletionResult]::new('-o', '-o', [CompletionResultType]::ParameterName, '(for usage with `--from-ref`) -- this option represents the destination ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch being pushed. For `post-checkout` hooks, this represents the branch that is now checked out.')
            [CompletionResult]::new('--to-ref', '--to-ref', [CompletionResultType]::ParameterName, '(for usage with `--from-ref`) -- this option represents the destination ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch being pushed. For `post-checkout` hooks, this represents the branch that is now checked out.')
            [CompletionResult]::new('--origin', '--origin', [CompletionResultType]::ParameterName, '(for usage with `--from-ref`) -- this option represents the destination ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch being pushed. For `post-checkout` hooks, this represents the branch that is now checked out.')
            [CompletionResult]::new('--pre-rebase-upstream', '--pre-rebase-upstream', [CompletionResultType]::ParameterName, 'The upstream from which the series was forked.')
            [CompletionResult]::new('--pre-rebase-branch', '--pre-rebase-branch', [CompletionResultType]::ParameterName, 'The branch being rebased, and is not set when rebasing the current branch.')
            [CompletionResult]::new('--commit-msg-filename', '--commit-msg-filename', [CompletionResultType]::ParameterName, 'Filename to check when running during `commit-msg`')
            [CompletionResult]::new('--prepare-commit-message-source', '--prepare-commit-message-source', [CompletionResultType]::ParameterName, 'Source of the commit message (typically the second argument to .git/hooks/prepare-commit-msg)')
            [CompletionResult]::new('--commit-object-name', '--commit-object-name', [CompletionResultType]::ParameterName, 'Commit object name (typically the third argument to .git/hooks/prepare-commit-msg)')
            [CompletionResult]::new('--remote-name', '--remote-name', [CompletionResultType]::ParameterName, 'Remote name used by `git push`.')
            [CompletionResult]::new('--remote-url', '--remote-url', [CompletionResultType]::ParameterName, 'Remote url used by `git push`.')
            [CompletionResult]::new('--checkout-type', '--checkout-type', [CompletionResultType]::ParameterName, 'Indicates whether the checkout was a branch checkout (changing branches, flag=1) or a file checkout (retrieving a file from the index, flag=0).')
            [CompletionResult]::new('--is-squash-merge', '--is-squash-merge', [CompletionResultType]::ParameterName, 'During a post-merge hook, indicates whether the merge was a squash merge')
            [CompletionResult]::new('--rewrite-command', '--rewrite-command', [CompletionResultType]::ParameterName, 'During a post-rewrite hook, specifies the command that invoked the rewrite')
            [CompletionResult]::new('-j', '-j', [CompletionResultType]::ParameterName, 'Number of hooks to run in parallel (default: the number of CPUs).')
            [CompletionResult]::new('--jobs', '--jobs', [CompletionResultType]::ParameterName, 'Number of hooks to run in parallel (default: the number of CPUs).')
            [CompletionResult]::new('--group', '--group', [CompletionResultType]::ParameterName, 'Run only hooks in this group; repeat to select hooks in any of the groups. `@ungrouped` selects hooks without groups.')
            [CompletionResult]::new('--require-group', '--require-group', [CompletionResultType]::ParameterName, 'Run only hooks in this group; repeat to require every group.')
            [CompletionResult]::new('--no-group', '--no-group', [CompletionResultType]::ParameterName, 'Skip hooks in this group (repeatable; wins over --group and --require-group).')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('-v', '-v', [CompletionResultType]::ParameterName, 'v')
            [CompletionResult]::new('--verbose', '--verbose', [CompletionResultType]::ParameterName, 'verbose')
            [CompletionResult]::new('-a', '-a', [CompletionResultType]::ParameterName, 'Run on all the files in the repo.')
            [CompletionResult]::new('--all-files', '--all-files', [CompletionResultType]::ParameterName, 'Run on all the files in the repo.')
            [CompletionResult]::new('--show-diff-on-failure', '--show-diff-on-failure', [CompletionResultType]::ParameterName, 'When hooks fail, run `git diff` directly afterward.')
            [CompletionResult]::new('--fail-fast', '--fail-fast', [CompletionResultType]::ParameterName, 'Stop after the first failing hook.')
            [CompletionResult]::new('--no-fast-path', '--no-fast-path', [CompletionResultType]::ParameterName, 'Run pre-commit-hooks in their Python environments instead of snp''s native implementations.')
            [CompletionResult]::new('--summary', '--summary', [CompletionResultType]::ParameterName, 'Print a summary of the hook results at the end.')
            [CompletionResult]::new('--no-local-config', '--no-local-config', [CompletionResultType]::ParameterName, 'Ignore the local override file (.pre-commit-config.local.yaml).')
            break
        }
        'snp;sample-config' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;self' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('alias', 'alias', [CompletionResultType]::ParameterValue, 'Manage the `pre-commit` alias.')
            [CompletionResult]::new('check', 'check', [CompletionResultType]::ParameterValue, 'Run the built-in self-test.')
            break
        }
        'snp;self;alias' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('install', 'install', [CompletionResultType]::ParameterValue, 'Create a `pre-commit` alias of snp.')
            [CompletionResult]::new('uninstall', 'uninstall', [CompletionResultType]::ParameterValue, 'Remove the `pre-commit` alias.')
            [CompletionResult]::new('status', 'status', [CompletionResultType]::ParameterValue, 'Show what `pre-commit` resolves to.')
            break
        }
        'snp;self;alias;install' {
            [CompletionResult]::new('--dir', '--dir', [CompletionResultType]::ParameterName, 'Directory of the alias (default: the directory of snp).')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('-f', '-f', [CompletionResultType]::ParameterName, 'Replace an existing pre-commit, keeping it as pre-commit.snp-backup.')
            [CompletionResult]::new('--force', '--force', [CompletionResultType]::ParameterName, 'Replace an existing pre-commit, keeping it as pre-commit.snp-backup.')
            break
        }
        'snp;self;alias;uninstall' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;self;alias;status' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;self;check' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;try-repo' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--ref', '--ref', [CompletionResultType]::ParameterName, 'Manually select a rev to run against, otherwise the `HEAD` revision will be used.')
            [CompletionResult]::new('--rev', '--rev', [CompletionResultType]::ParameterName, 'Manually select a rev to run against, otherwise the `HEAD` revision will be used.')
            [CompletionResult]::new('--files', '--files', [CompletionResultType]::ParameterName, 'Specific filenames to run hooks on.')
            [CompletionResult]::new('--stdin-files', '--stdin-files', [CompletionResultType]::ParameterName, 'Run on the file names read from stdin: one per line (`lines`, the default) or NUL-separated (`nul`).')
            [CompletionResult]::new('--hook-stage', '--hook-stage', [CompletionResultType]::ParameterName, 'The stage during which the hook is fired. One of commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg, manual')
            [CompletionResult]::new('--remote-branch', '--remote-branch', [CompletionResultType]::ParameterName, 'Remote branch ref used by `git push`.')
            [CompletionResult]::new('--local-branch', '--local-branch', [CompletionResultType]::ParameterName, 'Local branch ref used by `git push`.')
            [CompletionResult]::new('-s', '-s', [CompletionResultType]::ParameterName, '(for usage with `--to-ref`) -- this option represents the original ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch you are pushing to. For `post-checkout` hooks, this represents the branch that was previously checked out.')
            [CompletionResult]::new('--from-ref', '--from-ref', [CompletionResultType]::ParameterName, '(for usage with `--to-ref`) -- this option represents the original ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch you are pushing to. For `post-checkout` hooks, this represents the branch that was previously checked out.')
            [CompletionResult]::new('--source', '--source', [CompletionResultType]::ParameterName, '(for usage with `--to-ref`) -- this option represents the original ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch you are pushing to. For `post-checkout` hooks, this represents the branch that was previously checked out.')
            [CompletionResult]::new('-o', '-o', [CompletionResultType]::ParameterName, '(for usage with `--from-ref`) -- this option represents the destination ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch being pushed. For `post-checkout` hooks, this represents the branch that is now checked out.')
            [CompletionResult]::new('--to-ref', '--to-ref', [CompletionResultType]::ParameterName, '(for usage with `--from-ref`) -- this option represents the destination ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch being pushed. For `post-checkout` hooks, this represents the branch that is now checked out.')
            [CompletionResult]::new('--origin', '--origin', [CompletionResultType]::ParameterName, '(for usage with `--from-ref`) -- this option represents the destination ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch being pushed. For `post-checkout` hooks, this represents the branch that is now checked out.')
            [CompletionResult]::new('--pre-rebase-upstream', '--pre-rebase-upstream', [CompletionResultType]::ParameterName, 'The upstream from which the series was forked.')
            [CompletionResult]::new('--pre-rebase-branch', '--pre-rebase-branch', [CompletionResultType]::ParameterName, 'The branch being rebased, and is not set when rebasing the current branch.')
            [CompletionResult]::new('--commit-msg-filename', '--commit-msg-filename', [CompletionResultType]::ParameterName, 'Filename to check when running during `commit-msg`')
            [CompletionResult]::new('--prepare-commit-message-source', '--prepare-commit-message-source', [CompletionResultType]::ParameterName, 'Source of the commit message (typically the second argument to .git/hooks/prepare-commit-msg)')
            [CompletionResult]::new('--commit-object-name', '--commit-object-name', [CompletionResultType]::ParameterName, 'Commit object name (typically the third argument to .git/hooks/prepare-commit-msg)')
            [CompletionResult]::new('--remote-name', '--remote-name', [CompletionResultType]::ParameterName, 'Remote name used by `git push`.')
            [CompletionResult]::new('--remote-url', '--remote-url', [CompletionResultType]::ParameterName, 'Remote url used by `git push`.')
            [CompletionResult]::new('--checkout-type', '--checkout-type', [CompletionResultType]::ParameterName, 'Indicates whether the checkout was a branch checkout (changing branches, flag=1) or a file checkout (retrieving a file from the index, flag=0).')
            [CompletionResult]::new('--is-squash-merge', '--is-squash-merge', [CompletionResultType]::ParameterName, 'During a post-merge hook, indicates whether the merge was a squash merge')
            [CompletionResult]::new('--rewrite-command', '--rewrite-command', [CompletionResultType]::ParameterName, 'During a post-rewrite hook, specifies the command that invoked the rewrite')
            [CompletionResult]::new('-j', '-j', [CompletionResultType]::ParameterName, 'Number of hooks to run in parallel (default: the number of CPUs).')
            [CompletionResult]::new('--jobs', '--jobs', [CompletionResultType]::ParameterName, 'Number of hooks to run in parallel (default: the number of CPUs).')
            [CompletionResult]::new('--group', '--group', [CompletionResultType]::ParameterName, 'Run only hooks in this group; repeat to select hooks in any of the groups. `@ungrouped` selects hooks without groups.')
            [CompletionResult]::new('--require-group', '--require-group', [CompletionResultType]::ParameterName, 'Run only hooks in this group; repeat to require every group.')
            [CompletionResult]::new('--no-group', '--no-group', [CompletionResultType]::ParameterName, 'Skip hooks in this group (repeatable; wins over --group and --require-group).')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('-v', '-v', [CompletionResultType]::ParameterName, 'v')
            [CompletionResult]::new('--verbose', '--verbose', [CompletionResultType]::ParameterName, 'verbose')
            [CompletionResult]::new('-a', '-a', [CompletionResultType]::ParameterName, 'Run on all the files in the repo.')
            [CompletionResult]::new('--all-files', '--all-files', [CompletionResultType]::ParameterName, 'Run on all the files in the repo.')
            [CompletionResult]::new('--show-diff-on-failure', '--show-diff-on-failure', [CompletionResultType]::ParameterName, 'When hooks fail, run `git diff` directly afterward.')
            [CompletionResult]::new('--fail-fast', '--fail-fast', [CompletionResultType]::ParameterName, 'Stop after the first failing hook.')
            [CompletionResult]::new('--no-fast-path', '--no-fast-path', [CompletionResultType]::ParameterName, 'Run pre-commit-hooks in their Python environments instead of snp''s native implementations.')
            [CompletionResult]::new('--summary', '--summary', [CompletionResultType]::ParameterName, 'Print a summary of the hook results at the end.')
            [CompletionResult]::new('--no-local-config', '--no-local-config', [CompletionResultType]::ParameterName, 'Ignore the local override file (.pre-commit-config.local.yaml).')
            break
        }
        'snp;uninstall' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('-t', '-t', [CompletionResultType]::ParameterName, 't')
            [CompletionResult]::new('--hook-type', '--hook-type', [CompletionResultType]::ParameterName, 'hook-type')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--git-config-hooks', '--git-config-hooks', [CompletionResultType]::ParameterName, 'Remove the git config hook entries instead of hook scripts.')
            break
        }
        'snp;validate-config' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--strict', '--strict', [CompletionResultType]::ParameterName, 'Also report unknown hook keys (pre-commit only warns when it runs the hook).')
            [CompletionResult]::new('--portable', '--portable', [CompletionResultType]::ParameterName, 'Also list the snp extension keys that pre-commit would warn about.')
            break
        }
        'snp;validate-manifest' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--strict', '--strict', [CompletionResultType]::ParameterName, 'Also report unknown hook keys (pre-commit only warns when it runs the hook).')
            [CompletionResult]::new('--portable', '--portable', [CompletionResultType]::ParameterName, 'Also list the snp extension keys that pre-commit would warn about.')
            break
        }
        'snp;help' {
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            break
        }
        'snp;hook-impl' {
            [CompletionResult]::new('--color', '--color', [CompletionResultType]::ParameterName, 'Whether to use color in output. Defaults to `auto`.')
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Path to alternate config file')
            [CompletionResult]::new('--hook-type', '--hook-type', [CompletionResultType]::ParameterName, 'hook-type')
            [CompletionResult]::new('--hook-dir', '--hook-dir', [CompletionResultType]::ParameterName, 'hook-dir')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'show this help message and exit')
            [CompletionResult]::new('--skip-on-missing-config', '--skip-on-missing-config', [CompletionResultType]::ParameterName, 'skip-on-missing-config')
            break
        }
    })

    $completions.Where{ $_.CompletionText -like "$wordToComplete*" } |
        Sort-Object -Property ListItemText
}
