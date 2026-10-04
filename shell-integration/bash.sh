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

# --- Prompt marks (OSC 133) -------------------------------------------
# Let the terminal know when a command starts and when the prompt comes
# back, so it can warn before closing a tab with work still running.
_korterm_prompt_start() { printf '\033]133;A\007'; }
_korterm_command_start() { printf '\033]133;D\007'; }
case "$PROMPT_COMMAND" in
    *_korterm_prompt_start*) ;;
    "") PROMPT_COMMAND="_korterm_prompt_start" ;;
    *) PROMPT_COMMAND="${PROMPT_COMMAND%;};_korterm_prompt_start" ;;
esac
# `trap ... DEBUG` fires before every command, but not for the prompt
# machinery itself, which is exactly the boundary we want.
trap '_korterm_command_start' DEBUG