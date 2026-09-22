# Bash completion for mount-manager(1).
# Installed to /usr/share/bash-completion/completions/mount-manager

_mount_manager() {
    local cur prev words cword
    _init_completion || return

    local commands="list devices shares fstab discover mount add test umount mount-device remove auto-mount open mkdir info help version"
    local protocols="smb nfs sshfs webdav ftp block bind custom"
    local methods="auto system gvfs fuse"
    local escalations="auto pkexec sudo never"

    if [[ $cword -eq 1 ]]; then
        COMPREPLY=( $(compgen -W "$commands --json --all --help --version" -- "$cur") )
        return 0
    fi

    case "$prev" in
        --protocol|-t)   COMPREPLY=( $(compgen -W "$protocols" -- "$cur") ); return 0 ;;
        --method)        COMPREPLY=( $(compgen -W "$methods" -- "$cur") ); return 0 ;;
        --escalate)      COMPREPLY=( $(compgen -W "$escalations" -- "$cur") ); return 0 ;;
        --device|--target|--identity|--source)
            _filedir; return 0 ;;
        --host)          return 0 ;;
        --user|-u|--password|-p|--domain|--port|--name|--id|--option|-o|--path)
            return 0 ;;
    esac

    case "${words[1]}" in
        umount|unmount|open)
            # Offer currently mounted targets.
            local targets
            targets=$(awk '$2 != "/" { print $2 }' /proc/self/mountinfo 2>/dev/null | sort -u)
            COMPREPLY=( $(compgen -W "$targets" -- "$cur") )
            _filedir -d
            return 0 ;;
        mount-device)
            COMPREPLY=( $(compgen -W "$(ls /dev/sd* /dev/nvme*n* /dev/mmcblk* 2>/dev/null)" -- "$cur") )
            return 0 ;;
        mount|add|test)
            COMPREPLY=( $(compgen -W "--protocol --host --path --device --source --fstype --target --user --password --domain --port --identity --option --method --ro --persist --bookmark --remember-password --take-ownership --save --escalate" -- "$cur") )
            [[ $cur != -* ]] && _filedir -d
            return 0 ;;
        remove|rm)
            COMPREPLY=( $(compgen -W "$(mount-manager shares --json 2>/dev/null | sed -n 's/.*"id": "\(.*\)",/\1/p')" -- "$cur") )
            return 0 ;;
        *)
            COMPREPLY=( $(compgen -W "--json --all --lazy --restore --verify --quiet --escalate" -- "$cur") )
            return 0 ;;
    esac
} && complete -F _mount_manager mount-manager
