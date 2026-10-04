# Korterm shell integration — POSIX sh (dash, ash, busybox…)
# Sourced from ~/.profile by Korterm's "Shell" settings section.
#
# Plain sh has no programmable completion of its own, so this only loads
# bash-completion when the shell can actually drive it (`complete` is a
# bash/zsh builtin). Nothing here is required — it is a no-op on shells
# without completion support.

case $- in
    *i*) ;;
    *) return 0 ;;
esac

if command -v complete >/dev/null 2>&1; then
    for _korterm_f in \
        /usr/share/bash-completion/bash_completion \
        /etc/bash_completion
    do
        if [ -r "$_korterm_f" ]; then
            . "$_korterm_f"
            break
        fi
    done
    unset _korterm_f
fi