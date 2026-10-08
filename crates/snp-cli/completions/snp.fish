# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_snp_global_optspecs
    string join \n h/help V/version
end

function __fish_snp_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_snp_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_snp_using_subcommand
    set -l cmd (__fish_snp_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c snp -n "__fish_snp_needs_command" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_needs_command" -s V -l version -d 'show program\'s version number and exit'
complete -c snp -n "__fish_snp_needs_command" -f -a "autoupdate" -d 'Auto-update snp config to the latest repos\' versions.'
complete -c snp -n "__fish_snp_needs_command" -f -a "cache" -d 'Inspect or clean snp\'s cache.'
complete -c snp -n "__fish_snp_needs_command" -f -a "clean" -d 'Clean out snp files.'
complete -c snp -n "__fish_snp_needs_command" -f -a "completions" -d 'Print a shell completion script.'
complete -c snp -n "__fish_snp_needs_command" -f -a "config" -d 'Print the merged configuration or its JSON schema.'
complete -c snp -n "__fish_snp_needs_command" -f -a "doctor" -d 'Check snp, the hook scripts and the cache for problems.'
complete -c snp -n "__fish_snp_needs_command" -f -a "gc" -d 'Clean unused cached repos.'
complete -c snp -n "__fish_snp_needs_command" -f -a "hazmat" -d 'Composable tools for rare use in hook `entry`.'
complete -c snp -n "__fish_snp_needs_command" -f -a "init-templatedir" -d 'Install hook script in a directory intended for use with `git config init.templateDir`.'
complete -c snp -n "__fish_snp_needs_command" -f -a "install" -d 'Install the snp script.'
complete -c snp -n "__fish_snp_needs_command" -f -a "install-hooks" -d 'Install hook environments for all environments in the config file.  You may find `snp install --install-hooks` more useful.'
complete -c snp -n "__fish_snp_needs_command" -f -a "list" -d 'List the hooks of the configuration.'
complete -c snp -n "__fish_snp_needs_command" -f -a "migrate" -d 'Convert lefthook configurations or switch pre-commit-hooks to and from `repo: builtin`.'
complete -c snp -n "__fish_snp_needs_command" -f -a "migrate-config" -d 'Migrate list configuration to new map configuration.'
complete -c snp -n "__fish_snp_needs_command" -f -a "run" -d 'Run hooks.'
complete -c snp -n "__fish_snp_needs_command" -f -a "sample-config" -d 'Produce a sample .pre-commit-config.yaml file'
complete -c snp -n "__fish_snp_needs_command" -f -a "self" -d 'Manage the snp installation.'
complete -c snp -n "__fish_snp_needs_command" -f -a "try-repo" -d 'Try the hooks in a repository, useful for developing new hooks.'
complete -c snp -n "__fish_snp_needs_command" -f -a "uninstall" -d 'Uninstall the snp script.'
complete -c snp -n "__fish_snp_needs_command" -f -a "validate-config" -d 'Validate .pre-commit-config.yaml files'
complete -c snp -n "__fish_snp_needs_command" -f -a "validate-manifest" -d 'Validate .pre-commit-hooks.yaml files'
complete -c snp -n "__fish_snp_needs_command" -f -a "help" -d 'Show help for a specific command.'
complete -c snp -n "__fish_snp_needs_command" -f -a "hook-impl"
complete -c snp -n "__fish_snp_using_subcommand autoupdate" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand autoupdate" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand autoupdate" -l repo -d 'Only update this repository -- may be specified multiple times.' -r
complete -c snp -n "__fish_snp_using_subcommand autoupdate" -s j -l jobs -d 'Number of threads to use. (default 1).' -r
complete -c snp -n "__fish_snp_using_subcommand autoupdate" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand autoupdate" -l bleeding-edge -d 'Update to the bleeding edge of `HEAD` instead of the latest tagged version (the default behavior).'
complete -c snp -n "__fish_snp_using_subcommand autoupdate" -l freeze -d 'Store "frozen" hashes in `rev` instead of tag names'
complete -c snp -n "__fish_snp_using_subcommand cache; and not __fish_seen_subcommand_from dir size gc clean" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand cache; and not __fish_seen_subcommand_from dir size gc clean" -f -a "dir" -d 'Print the cache directory.'
complete -c snp -n "__fish_snp_using_subcommand cache; and not __fish_seen_subcommand_from dir size gc clean" -f -a "size" -d 'Print the size of the cache.'
complete -c snp -n "__fish_snp_using_subcommand cache; and not __fish_seen_subcommand_from dir size gc clean" -f -a "gc" -d 'Remove unused repositories and environments.'
complete -c snp -n "__fish_snp_using_subcommand cache; and not __fish_seen_subcommand_from dir size gc clean" -f -a "clean" -d 'Remove the whole cache.'
complete -c snp -n "__fish_snp_using_subcommand cache; and __fish_seen_subcommand_from dir" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand cache; and __fish_seen_subcommand_from size" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand cache; and __fish_seen_subcommand_from gc" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand cache; and __fish_seen_subcommand_from clean" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand clean" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand clean" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand completions" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand config; and not __fish_seen_subcommand_from dump schema" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand config; and not __fish_seen_subcommand_from dump schema" -f -a "dump" -d 'Print the merged configuration, local overrides included.'
complete -c snp -n "__fish_snp_using_subcommand config; and not __fish_seen_subcommand_from dump schema" -f -a "schema" -d 'Print the JSON schema of .pre-commit-config.yaml.'
complete -c snp -n "__fish_snp_using_subcommand config; and __fish_seen_subcommand_from dump" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand config; and __fish_seen_subcommand_from dump" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand config; and __fish_seen_subcommand_from dump" -l json -d 'Print JSON instead of YAML.'
complete -c snp -n "__fish_snp_using_subcommand config; and __fish_seen_subcommand_from dump" -l no-local-config -d 'Ignore the local override file (.pre-commit-config.local.yaml).'
complete -c snp -n "__fish_snp_using_subcommand config; and __fish_seen_subcommand_from schema" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand doctor" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand doctor" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand doctor" -l fix -d 'Repair this repository\'s hook scripts, then check again.'
complete -c snp -n "__fish_snp_using_subcommand doctor" -l fast-path -d 'Explain the native fast-path decision of every hook.'
complete -c snp -n "__fish_snp_using_subcommand doctor" -l stash -d 'Verify that a stash of unstaged changes round-trips.'
complete -c snp -n "__fish_snp_using_subcommand gc" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand gc" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand hazmat; and not __fish_seen_subcommand_from cd ignore-exit-code n1" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand hazmat; and not __fish_seen_subcommand_from cd ignore-exit-code n1" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand hazmat; and not __fish_seen_subcommand_from cd ignore-exit-code n1" -f -a "cd" -d 'cd to a subdir and run the command'
complete -c snp -n "__fish_snp_using_subcommand hazmat; and not __fish_seen_subcommand_from cd ignore-exit-code n1" -f -a "ignore-exit-code" -d 'run the command but ignore the exit code'
complete -c snp -n "__fish_snp_using_subcommand hazmat; and not __fish_seen_subcommand_from cd ignore-exit-code n1" -f -a "n1" -d 'run the command once per filename'
complete -c snp -n "__fish_snp_using_subcommand hazmat; and __fish_seen_subcommand_from cd" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand hazmat; and __fish_seen_subcommand_from ignore-exit-code" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand hazmat; and __fish_seen_subcommand_from n1" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand init-templatedir" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand init-templatedir" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand init-templatedir" -s t -l hook-type -r -f -a "commit-msg\t''
post-checkout\t''
post-commit\t''
post-merge\t''
post-rewrite\t''
pre-commit\t''
pre-merge-commit\t''
pre-push\t''
pre-rebase\t''
prepare-commit-msg\t''"
complete -c snp -n "__fish_snp_using_subcommand init-templatedir" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand init-templatedir" -l no-allow-missing-config -d 'Assume cloned repos should have a `pre-commit` config.'
complete -c snp -n "__fish_snp_using_subcommand install" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand install" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand install" -s t -l hook-type -r -f -a "commit-msg\t''
post-checkout\t''
post-commit\t''
post-merge\t''
post-rewrite\t''
pre-commit\t''
pre-merge-commit\t''
pre-push\t''
pre-rebase\t''
prepare-commit-msg\t''"
complete -c snp -n "__fish_snp_using_subcommand install" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand install" -s f -l overwrite -d 'Overwrite existing hooks / remove migration mode.'
complete -c snp -n "__fish_snp_using_subcommand install" -l install-hooks -d 'Whether to install hook environments for all environments in the config file.'
complete -c snp -n "__fish_snp_using_subcommand install" -l allow-missing-config -d 'Whether to allow a missing `snp` configuration file or exit with a failure code.'
complete -c snp -n "__fish_snp_using_subcommand install" -l git-config-hooks -d 'Install the hooks as git config entries (`hook.<name>.command`, git 2.54+) instead of hook scripts.'
complete -c snp -n "__fish_snp_using_subcommand install-hooks" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand install-hooks" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand install-hooks" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand install-hooks" -l with-fallback -d 'Also install the Python environments that hooks run natively by snp would fall back to (for offline use with SNP_NO_FAST_PATH).'
complete -c snp -n "__fish_snp_using_subcommand list" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand list" -l hook-stage -d 'Only list the hooks of this stage. One of commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg, manual' -r -f -a "commit-msg\t''
post-checkout\t''
post-commit\t''
post-merge\t''
post-rewrite\t''
pre-commit\t''
pre-merge-commit\t''
pre-push\t''
pre-rebase\t''
prepare-commit-msg\t''
manual\t''"
complete -c snp -n "__fish_snp_using_subcommand list" -l group -d 'Run only hooks in this group; repeat to select hooks in any of the groups. `@ungrouped` selects hooks without groups.' -r
complete -c snp -n "__fish_snp_using_subcommand list" -l require-group -d 'Run only hooks in this group; repeat to require every group.' -r
complete -c snp -n "__fish_snp_using_subcommand list" -l no-group -d 'Skip hooks in this group (repeatable; wins over --group and --require-group).' -r
complete -c snp -n "__fish_snp_using_subcommand list" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand list" -l json -d 'Print JSON instead of text.'
complete -c snp -n "__fish_snp_using_subcommand migrate; and not __fish_seen_subcommand_from lefthook builtin upstream" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand migrate; and not __fish_seen_subcommand_from lefthook builtin upstream" -f -a "lefthook" -d 'Convert a lefthook configuration.'
complete -c snp -n "__fish_snp_using_subcommand migrate; and not __fish_seen_subcommand_from lefthook builtin upstream" -f -a "builtin" -d 'Switch pre-commit-hooks repos to `repo: builtin`.'
complete -c snp -n "__fish_snp_using_subcommand migrate; and not __fish_seen_subcommand_from lefthook builtin upstream" -f -a "upstream" -d 'Switch `repo: builtin` back to pre-commit-hooks.'
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from lefthook" -l root -d 'Repository root (default: the current git repository).' -r
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from lefthook" -s c -l config -d 'lefthook configuration to convert (default: lefthook\'s lookup).' -r
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from lefthook" -s o -l output -d 'Where to write the result (default: .pre-commit-config.yaml; `-` for stdout).' -r
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from lefthook" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from lefthook" -l portable -d 'Emit only keys pre-commit 4.x understands.'
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from lefthook" -l local -d 'Also convert lefthook-local.yml into .pre-commit-config.local.yaml.'
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from lefthook" -l strict -d 'Fail (exit 2) when a feature cannot be converted faithfully.'
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from lefthook" -l install -d 'Afterwards remove lefthook\'s hooks and install snp\'s.'
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from lefthook" -s f -l force -d 'Overwrite an existing output file.'
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from builtin" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from builtin" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from upstream" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand migrate; and __fish_seen_subcommand_from upstream" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand migrate-config" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand migrate-config" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand migrate-config" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand run" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l files -d 'Specific filenames to run hooks on.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l stdin-files -d 'Run on the file names read from stdin: one per line (`lines`, the default) or NUL-separated (`nul`).' -r -f -a "nul\t''
lines\t''"
complete -c snp -n "__fish_snp_using_subcommand run" -l hook-stage -d 'The stage during which the hook is fired. One of commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg, manual' -r -f -a "commit-msg\t''
post-checkout\t''
post-commit\t''
post-merge\t''
post-rewrite\t''
pre-commit\t''
pre-merge-commit\t''
pre-push\t''
pre-rebase\t''
prepare-commit-msg\t''
manual\t''"
complete -c snp -n "__fish_snp_using_subcommand run" -l remote-branch -d 'Remote branch ref used by `git push`.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l local-branch -d 'Local branch ref used by `git push`.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -s s -l from-ref -l source -d '(for usage with `--to-ref`) -- this option represents the original ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch you are pushing to. For `post-checkout` hooks, this represents the branch that was previously checked out.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -s o -l to-ref -l origin -d '(for usage with `--from-ref`) -- this option represents the destination ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch being pushed. For `post-checkout` hooks, this represents the branch that is now checked out.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l pre-rebase-upstream -d 'The upstream from which the series was forked.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l pre-rebase-branch -d 'The branch being rebased, and is not set when rebasing the current branch.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l commit-msg-filename -d 'Filename to check when running during `commit-msg`' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l prepare-commit-message-source -d 'Source of the commit message (typically the second argument to .git/hooks/prepare-commit-msg)' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l commit-object-name -d 'Commit object name (typically the third argument to .git/hooks/prepare-commit-msg)' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l remote-name -d 'Remote name used by `git push`.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l remote-url -d 'Remote url used by `git push`.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l checkout-type -d 'Indicates whether the checkout was a branch checkout (changing branches, flag=1) or a file checkout (retrieving a file from the index, flag=0).' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l is-squash-merge -d 'During a post-merge hook, indicates whether the merge was a squash merge' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l rewrite-command -d 'During a post-rewrite hook, specifies the command that invoked the rewrite' -r
complete -c snp -n "__fish_snp_using_subcommand run" -s j -l jobs -d 'Number of hooks to run in parallel (default: the number of CPUs).' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l group -d 'Run only hooks in this group; repeat to select hooks in any of the groups. `@ungrouped` selects hooks without groups.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l require-group -d 'Run only hooks in this group; repeat to require every group.' -r
complete -c snp -n "__fish_snp_using_subcommand run" -l no-group -d 'Skip hooks in this group (repeatable; wins over --group and --require-group).' -r
complete -c snp -n "__fish_snp_using_subcommand run" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand run" -s v -l verbose
complete -c snp -n "__fish_snp_using_subcommand run" -s a -l all-files -d 'Run on all the files in the repo.'
complete -c snp -n "__fish_snp_using_subcommand run" -l show-diff-on-failure -d 'When hooks fail, run `git diff` directly afterward.'
complete -c snp -n "__fish_snp_using_subcommand run" -l fail-fast -d 'Stop after the first failing hook.'
complete -c snp -n "__fish_snp_using_subcommand run" -l no-fast-path -d 'Run pre-commit-hooks in their Python environments instead of snp\'s native implementations.'
complete -c snp -n "__fish_snp_using_subcommand run" -l summary -d 'Print a summary of the hook results at the end.'
complete -c snp -n "__fish_snp_using_subcommand run" -l no-local-config -d 'Ignore the local override file (.pre-commit-config.local.yaml).'
complete -c snp -n "__fish_snp_using_subcommand sample-config" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand sample-config" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand self; and not __fish_seen_subcommand_from alias check" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand self; and not __fish_seen_subcommand_from alias check" -f -a "alias" -d 'Manage the `pre-commit` alias.'
complete -c snp -n "__fish_snp_using_subcommand self; and not __fish_seen_subcommand_from alias check" -f -a "check" -d 'Run the built-in self-test.'
complete -c snp -n "__fish_snp_using_subcommand self; and __fish_seen_subcommand_from alias" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand self; and __fish_seen_subcommand_from alias" -f -a "install" -d 'Create a `pre-commit` alias of snp.'
complete -c snp -n "__fish_snp_using_subcommand self; and __fish_seen_subcommand_from alias" -f -a "uninstall" -d 'Remove the `pre-commit` alias.'
complete -c snp -n "__fish_snp_using_subcommand self; and __fish_seen_subcommand_from alias" -f -a "status" -d 'Show what `pre-commit` resolves to.'
complete -c snp -n "__fish_snp_using_subcommand self; and __fish_seen_subcommand_from check" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l ref -l rev -d 'Manually select a rev to run against, otherwise the `HEAD` revision will be used.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l files -d 'Specific filenames to run hooks on.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l stdin-files -d 'Run on the file names read from stdin: one per line (`lines`, the default) or NUL-separated (`nul`).' -r -f -a "nul\t''
lines\t''"
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l hook-stage -d 'The stage during which the hook is fired. One of commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg, manual' -r -f -a "commit-msg\t''
post-checkout\t''
post-commit\t''
post-merge\t''
post-rewrite\t''
pre-commit\t''
pre-merge-commit\t''
pre-push\t''
pre-rebase\t''
prepare-commit-msg\t''
manual\t''"
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l remote-branch -d 'Remote branch ref used by `git push`.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l local-branch -d 'Local branch ref used by `git push`.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -s s -l from-ref -l source -d '(for usage with `--to-ref`) -- this option represents the original ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch you are pushing to. For `post-checkout` hooks, this represents the branch that was previously checked out.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -s o -l to-ref -l origin -d '(for usage with `--from-ref`) -- this option represents the destination ref in a `from_ref...to_ref` diff expression. For `pre-push` hooks, this represents the branch being pushed. For `post-checkout` hooks, this represents the branch that is now checked out.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l pre-rebase-upstream -d 'The upstream from which the series was forked.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l pre-rebase-branch -d 'The branch being rebased, and is not set when rebasing the current branch.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l commit-msg-filename -d 'Filename to check when running during `commit-msg`' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l prepare-commit-message-source -d 'Source of the commit message (typically the second argument to .git/hooks/prepare-commit-msg)' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l commit-object-name -d 'Commit object name (typically the third argument to .git/hooks/prepare-commit-msg)' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l remote-name -d 'Remote name used by `git push`.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l remote-url -d 'Remote url used by `git push`.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l checkout-type -d 'Indicates whether the checkout was a branch checkout (changing branches, flag=1) or a file checkout (retrieving a file from the index, flag=0).' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l is-squash-merge -d 'During a post-merge hook, indicates whether the merge was a squash merge' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l rewrite-command -d 'During a post-rewrite hook, specifies the command that invoked the rewrite' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -s j -l jobs -d 'Number of hooks to run in parallel (default: the number of CPUs).' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l group -d 'Run only hooks in this group; repeat to select hooks in any of the groups. `@ungrouped` selects hooks without groups.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l require-group -d 'Run only hooks in this group; repeat to require every group.' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l no-group -d 'Skip hooks in this group (repeatable; wins over --group and --require-group).' -r
complete -c snp -n "__fish_snp_using_subcommand try-repo" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand try-repo" -s v -l verbose
complete -c snp -n "__fish_snp_using_subcommand try-repo" -s a -l all-files -d 'Run on all the files in the repo.'
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l show-diff-on-failure -d 'When hooks fail, run `git diff` directly afterward.'
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l fail-fast -d 'Stop after the first failing hook.'
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l no-fast-path -d 'Run pre-commit-hooks in their Python environments instead of snp\'s native implementations.'
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l summary -d 'Print a summary of the hook results at the end.'
complete -c snp -n "__fish_snp_using_subcommand try-repo" -l no-local-config -d 'Ignore the local override file (.pre-commit-config.local.yaml).'
complete -c snp -n "__fish_snp_using_subcommand uninstall" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand uninstall" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand uninstall" -s t -l hook-type -r -f -a "commit-msg\t''
post-checkout\t''
post-commit\t''
post-merge\t''
post-rewrite\t''
pre-commit\t''
pre-merge-commit\t''
pre-push\t''
pre-rebase\t''
prepare-commit-msg\t''"
complete -c snp -n "__fish_snp_using_subcommand uninstall" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand uninstall" -l git-config-hooks -d 'Remove the git config hook entries instead of hook scripts.'
complete -c snp -n "__fish_snp_using_subcommand validate-config" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand validate-config" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand validate-config" -l strict -d 'Also report unknown hook keys (pre-commit only warns when it runs the hook).'
complete -c snp -n "__fish_snp_using_subcommand validate-config" -l portable -d 'Also list the snp extension keys that pre-commit would warn about.'
complete -c snp -n "__fish_snp_using_subcommand validate-manifest" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand validate-manifest" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand validate-manifest" -l strict -d 'Also report unknown hook keys (pre-commit only warns when it runs the hook).'
complete -c snp -n "__fish_snp_using_subcommand validate-manifest" -l portable -d 'Also list the snp extension keys that pre-commit would warn about.'
complete -c snp -n "__fish_snp_using_subcommand help" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand hook-impl" -l color -d 'Whether to use color in output. Defaults to `auto`.' -r
complete -c snp -n "__fish_snp_using_subcommand hook-impl" -s c -l config -d 'Path to alternate config file' -r
complete -c snp -n "__fish_snp_using_subcommand hook-impl" -l hook-type -r
complete -c snp -n "__fish_snp_using_subcommand hook-impl" -l hook-dir -r
complete -c snp -n "__fish_snp_using_subcommand hook-impl" -s h -l help -d 'show this help message and exit'
complete -c snp -n "__fish_snp_using_subcommand hook-impl" -l skip-on-missing-config
