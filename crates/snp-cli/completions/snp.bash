_snp() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="snp"
                ;;
            snp,autoupdate)
                cmd="snp__subcmd__autoupdate"
                ;;
            snp,cache)
                cmd="snp__subcmd__cache"
                ;;
            snp,clean)
                cmd="snp__subcmd__clean"
                ;;
            snp,completions)
                cmd="snp__subcmd__completions"
                ;;
            snp,config)
                cmd="snp__subcmd__config"
                ;;
            snp,doctor)
                cmd="snp__subcmd__doctor"
                ;;
            snp,gc)
                cmd="snp__subcmd__gc"
                ;;
            snp,hazmat)
                cmd="snp__subcmd__hazmat"
                ;;
            snp,help)
                cmd="snp__subcmd__help"
                ;;
            snp,hook-impl)
                cmd="snp__subcmd__hook__subcmd__impl"
                ;;
            snp,init-templatedir)
                cmd="snp__subcmd__init__subcmd__templatedir"
                ;;
            snp,install)
                cmd="snp__subcmd__install"
                ;;
            snp,install-hooks)
                cmd="snp__subcmd__install__subcmd__hooks"
                ;;
            snp,list)
                cmd="snp__subcmd__list"
                ;;
            snp,migrate)
                cmd="snp__subcmd__migrate"
                ;;
            snp,migrate-config)
                cmd="snp__subcmd__migrate__subcmd__config"
                ;;
            snp,run)
                cmd="snp__subcmd__run"
                ;;
            snp,sample-config)
                cmd="snp__subcmd__sample__subcmd__config"
                ;;
            snp,self)
                cmd="snp__subcmd__self"
                ;;
            snp,try-repo)
                cmd="snp__subcmd__try__subcmd__repo"
                ;;
            snp,uninstall)
                cmd="snp__subcmd__uninstall"
                ;;
            snp,validate-config)
                cmd="snp__subcmd__validate__subcmd__config"
                ;;
            snp,validate-manifest)
                cmd="snp__subcmd__validate__subcmd__manifest"
                ;;
            snp__subcmd__cache,clean)
                cmd="snp__subcmd__cache__subcmd__clean"
                ;;
            snp__subcmd__cache,dir)
                cmd="snp__subcmd__cache__subcmd__dir"
                ;;
            snp__subcmd__cache,gc)
                cmd="snp__subcmd__cache__subcmd__gc"
                ;;
            snp__subcmd__cache,size)
                cmd="snp__subcmd__cache__subcmd__size"
                ;;
            snp__subcmd__config,dump)
                cmd="snp__subcmd__config__subcmd__dump"
                ;;
            snp__subcmd__config,schema)
                cmd="snp__subcmd__config__subcmd__schema"
                ;;
            snp__subcmd__hazmat,cd)
                cmd="snp__subcmd__hazmat__subcmd__cd"
                ;;
            snp__subcmd__hazmat,ignore-exit-code)
                cmd="snp__subcmd__hazmat__subcmd__ignore__subcmd__exit__subcmd__code"
                ;;
            snp__subcmd__hazmat,n1)
                cmd="snp__subcmd__hazmat__subcmd__n1"
                ;;
            snp__subcmd__migrate,builtin)
                cmd="snp__subcmd__migrate__subcmd__builtin"
                ;;
            snp__subcmd__migrate,lefthook)
                cmd="snp__subcmd__migrate__subcmd__lefthook"
                ;;
            snp__subcmd__migrate,upstream)
                cmd="snp__subcmd__migrate__subcmd__upstream"
                ;;
            snp__subcmd__self,alias)
                cmd="snp__subcmd__self__subcmd__alias"
                ;;
            snp__subcmd__self,check)
                cmd="snp__subcmd__self__subcmd__check"
                ;;
            snp__subcmd__self__subcmd__alias,install)
                cmd="snp__subcmd__self__subcmd__alias__subcmd__install"
                ;;
            snp__subcmd__self__subcmd__alias,status)
                cmd="snp__subcmd__self__subcmd__alias__subcmd__status"
                ;;
            snp__subcmd__self__subcmd__alias,uninstall)
                cmd="snp__subcmd__self__subcmd__alias__subcmd__uninstall"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        snp)
            opts="-h -V --help --version autoupdate cache clean completions config doctor gc hazmat init-templatedir install install-hooks list migrate migrate-config run sample-config self try-repo uninstall validate-config validate-manifest help hook-impl"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__autoupdate)
            opts="-h -c -j --help --color --config --bleeding-edge --freeze --repo --jobs"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --repo)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --jobs)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -j)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__cache)
            opts="-h --help dir size gc clean"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__cache__subcmd__clean)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__cache__subcmd__dir)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__cache__subcmd__gc)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__cache__subcmd__size)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__clean)
            opts="-h --help --color"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__completions)
            opts="-h --help bash zsh fish powershell"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__config)
            opts="-h --help dump schema"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__config__subcmd__dump)
            opts="-h -c --help --config --json --no-local-config"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__config__subcmd__schema)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__doctor)
            opts="-h -c --help --config --fix --fast-path --stash"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__gc)
            opts="-h --help --color"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__hazmat)
            opts="-h --help --color cd ignore-exit-code n1"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__hazmat__subcmd__cd)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__hazmat__subcmd__ignore__subcmd__exit__subcmd__code)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__hazmat__subcmd__n1)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__help)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__hook__subcmd__impl)
            opts="-h -c --help --color --config --hook-type --hook-dir --skip-on-missing-config"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --hook-type)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --hook-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__init__subcmd__templatedir)
            opts="-h -c -t --help --color --config --no-allow-missing-config --hook-type"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --hook-type)
                    COMPREPLY=($(compgen -W "commit-msg post-checkout post-commit post-merge post-rewrite pre-commit pre-merge-commit pre-push pre-rebase prepare-commit-msg" -- "${cur}"))
                    return 0
                    ;;
                -t)
                    COMPREPLY=($(compgen -W "commit-msg post-checkout post-commit post-merge post-rewrite pre-commit pre-merge-commit pre-push pre-rebase prepare-commit-msg" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__install)
            opts="-h -c -f -t --help --color --config --overwrite --install-hooks --hook-type --allow-missing-config --git-config-hooks"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --hook-type)
                    COMPREPLY=($(compgen -W "commit-msg post-checkout post-commit post-merge post-rewrite pre-commit pre-merge-commit pre-push pre-rebase prepare-commit-msg" -- "${cur}"))
                    return 0
                    ;;
                -t)
                    COMPREPLY=($(compgen -W "commit-msg post-checkout post-commit post-merge post-rewrite pre-commit pre-merge-commit pre-push pre-rebase prepare-commit-msg" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__install__subcmd__hooks)
            opts="-h -c --help --color --config --with-fallback"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__list)
            opts="-h -c --help --config --json --hook-stage --group --require-group --no-group"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --hook-stage)
                    COMPREPLY=($(compgen -W "commit-msg post-checkout post-commit post-merge post-rewrite pre-commit pre-merge-commit pre-push pre-rebase prepare-commit-msg manual" -- "${cur}"))
                    return 0
                    ;;
                --group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --require-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --no-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__migrate)
            opts="-h --help lefthook builtin upstream"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__migrate__subcmd__config)
            opts="-h -c --help --color --config"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__migrate__subcmd__builtin)
            opts="-h -c --help --config"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__migrate__subcmd__lefthook)
            opts="-h -c -o -f --help --root --config --output --portable --local --strict --install --force"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --root)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -o)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__migrate__subcmd__upstream)
            opts="-h -c --help --config"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__run)
            opts="-h -c -v -a -s -o -j --help --color --config --verbose --all-files --files --stdin-files --show-diff-on-failure --fail-fast --hook-stage --remote-branch --local-branch --source --from-ref --origin --to-ref --pre-rebase-upstream --pre-rebase-branch --commit-msg-filename --prepare-commit-message-source --commit-object-name --remote-name --remote-url --checkout-type --is-squash-merge --rewrite-command --jobs --group --require-group --no-group --no-fast-path --summary --no-local-config"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --files)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --stdin-files)
                    COMPREPLY=($(compgen -W "nul lines" -- "${cur}"))
                    return 0
                    ;;
                --hook-stage)
                    COMPREPLY=($(compgen -W "commit-msg post-checkout post-commit post-merge post-rewrite pre-commit pre-merge-commit pre-push pre-rebase prepare-commit-msg manual" -- "${cur}"))
                    return 0
                    ;;
                --remote-branch)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --local-branch)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --from-ref)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --source)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -s)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --to-ref)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --origin)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -o)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pre-rebase-upstream)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pre-rebase-branch)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --commit-msg-filename)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --prepare-commit-message-source)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --commit-object-name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --remote-name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --remote-url)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --checkout-type)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --is-squash-merge)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --rewrite-command)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --jobs)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -j)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --require-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --no-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__sample__subcmd__config)
            opts="-h --help --color"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__self)
            opts="-h --help alias check"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__self__subcmd__alias)
            opts="-h --help install uninstall status"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__self__subcmd__alias__subcmd__install)
            opts="-h -f --help --dir --force"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__self__subcmd__alias__subcmd__status)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__self__subcmd__alias__subcmd__uninstall)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__self__subcmd__check)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__try__subcmd__repo)
            opts="-h -c -v -a -s -o -j --help --color --config --rev --ref --verbose --all-files --files --stdin-files --show-diff-on-failure --fail-fast --hook-stage --remote-branch --local-branch --source --from-ref --origin --to-ref --pre-rebase-upstream --pre-rebase-branch --commit-msg-filename --prepare-commit-message-source --commit-object-name --remote-name --remote-url --checkout-type --is-squash-merge --rewrite-command --jobs --group --require-group --no-group --no-fast-path --summary --no-local-config"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --ref)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --rev)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --files)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --stdin-files)
                    COMPREPLY=($(compgen -W "nul lines" -- "${cur}"))
                    return 0
                    ;;
                --hook-stage)
                    COMPREPLY=($(compgen -W "commit-msg post-checkout post-commit post-merge post-rewrite pre-commit pre-merge-commit pre-push pre-rebase prepare-commit-msg manual" -- "${cur}"))
                    return 0
                    ;;
                --remote-branch)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --local-branch)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --from-ref)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --source)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -s)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --to-ref)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --origin)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -o)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pre-rebase-upstream)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pre-rebase-branch)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --commit-msg-filename)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --prepare-commit-message-source)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --commit-object-name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --remote-name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --remote-url)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --checkout-type)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --is-squash-merge)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --rewrite-command)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --jobs)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -j)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --require-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --no-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__uninstall)
            opts="-h -c -t --help --color --config --hook-type --git-config-hooks"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --hook-type)
                    COMPREPLY=($(compgen -W "commit-msg post-checkout post-commit post-merge post-rewrite pre-commit pre-merge-commit pre-push pre-rebase prepare-commit-msg" -- "${cur}"))
                    return 0
                    ;;
                -t)
                    COMPREPLY=($(compgen -W "commit-msg post-checkout post-commit post-merge post-rewrite pre-commit pre-merge-commit pre-push pre-rebase prepare-commit-msg" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__validate__subcmd__config)
            opts="-h --help --color --strict --portable"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        snp__subcmd__validate__subcmd__manifest)
            opts="-h --help --color --strict --portable"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --color)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _snp -o nosort -o bashdefault -o default snp
else
    complete -F _snp -o bashdefault -o default snp
fi
