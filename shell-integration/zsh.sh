# Korterm shell integration — zsh
# Sourced from ~/.zshrc by Korterm's "Shell" settings section.
# Everything here is opt-in and removable; it only loads components that
# are actually installed on this machine.

[[ -o interactive ]] || return 0

# _korterm_load <file> <dir>... — source the first readable <dir>/<file>.
_korterm_load() {
    local name="$1" dir
    shift
    for dir in "$@"; do
        [[ -r "$dir/$name" ]] && { source "$dir/$name"; return 0 }
    done
    return 1
}

# --- 1. Autosuggestions -------------------------------------------------
# Inline "ghost text" suggestions (fish-style). Must come before
# compinit so its widgets wrap the completion system.
_korterm_load zsh-autosuggestions.zsh \
    "${ZSH:-$HOME/.oh-my-zsh}/custom/plugins/zsh-autosuggestions" \
    /usr/share/zsh-autosuggestions \
    /usr/local/share/zsh-autosuggestions \
    "$HOME/.local/share/zsh-autosuggestions"

# --- 2. Completion ------------------------------------------------------
# Building the completion database is the expensive part (~1s the first
# time), so it is cached under the XDG cache dir and reused afterwards.
autoload -Uz compinit
_korterm_zcompdump="${XDG_CACHE_HOME:-$HOME/.cache}/korterm/zcompdump"
mkdir -p -m 700 "${_korterm_zcompdump:h}" 2>/dev/null
compinit -u -d "$_korterm_zcompdump"

# Menu-style completion: Tab opens a list to pick from instead of
# cycling blindly through every candidate sharing the prefix.
zstyle ':completion:*' menu select
zstyle ':completion:*' use-cache on
zstyle ':completion:*' cache-path "${XDG_CACHE_HOME:-$HOME/.cache}/korterm/zcompcache"
zstyle ':completion:*' matcher-list \
    'm:{a-zA-Z}={A-Za-z}' \
    'r:|=*' 'l:|=* r:|=*'
setopt AUTO_MENU COMPLETE_IN_WORD ALWAYS_TO_END

# --- 3. Syntax highlighting (last) -------------------------------------
# zsh-syntax-highlighting requires being sourced after compinit, so this
# block stays at the bottom of the file on purpose.
_korterm_load zsh-syntax-highlighting.zsh \
    "${ZSH:-$HOME/.oh-my-zsh}/custom/plugins/zsh-syntax-highlighting" \
    /usr/share/zsh-syntax-highlighting \
    /usr/local/share/zsh-syntax-highlighting \
    "$HOME/.local/share/zsh-syntax-highlighting"

unfunction _korterm_load 2>/dev/null
unset _korterm_zcompdump 2>/dev/null