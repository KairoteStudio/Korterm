# Korterm shell integration — bash
# Sourced from ~/.bashrc by Korterm's "Shell" settings section.
# Safe to delete: it only loads bash-completion when it is installed.

case $- in
    *i*) ;;
    *) return 0 ;;
esac

if [[ -r /usr/share/bash-completion/bash_completion ]]; then
    . /usr/share/bash-completion/bash_completion
elif [[ -r /etc/bash_completion ]]; then
    . /etc/bash_completion
fi

# Case-insensitive completion and a visible menu — the defaults hide
# candidates until the prefix is unambiguous.
bind 'set completion-ignore-case on' 2>/dev/null
bind 'set show-all-if-ambiguous on' 2>/dev/null
bind 'set menu-complete-display-prefix on' 2>/dev/null
bind 'set page-completions off' 2>/dev/null