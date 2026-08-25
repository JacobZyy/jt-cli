# jt cli bootstrap: Zsh config
export PATH="$HOME/.local/bin:$PATH"
if [[ -d /opt/homebrew ]]; then
    export PATH="/opt/homebrew/bin:/opt/homebrew/sbin:$PATH"
    JT_BREW_PREFIX="/opt/homebrew"
elif [[ -d /usr/local/Homebrew ]]; then
    export PATH="/usr/local/bin:/usr/local/sbin:$PATH"
    JT_BREW_PREFIX="/usr/local"
elif [[ -d /home/linuxbrew/.linuxbrew ]]; then
    export PATH="/home/linuxbrew/.linuxbrew/bin:/home/linuxbrew/.linuxbrew/sbin:$PATH"
    JT_BREW_PREFIX="/home/linuxbrew/.linuxbrew"
else
    JT_BREW_PREFIX=""
fi

export STARSHIP_CONFIG="$HOME/.config/jt-cli/starship.toml"
if command -v starship >/dev/null 2>&1; then
    eval "$(starship init zsh)"
fi

if [[ -n "$JT_BREW_PREFIX" && -f "$JT_BREW_PREFIX/share/zsh-autosuggestions/zsh-autosuggestions.zsh" ]]; then
    source "$JT_BREW_PREFIX/share/zsh-autosuggestions/zsh-autosuggestions.zsh"
elif [[ -f /usr/share/zsh-autosuggestions/zsh-autosuggestions.zsh ]]; then
    source /usr/share/zsh-autosuggestions/zsh-autosuggestions.zsh
fi

if [[ -n "$JT_BREW_PREFIX" && -d "$JT_BREW_PREFIX/share/zsh-completions" ]]; then
    fpath=("$JT_BREW_PREFIX/share/zsh-completions" $fpath)
elif [[ -d /usr/share/zsh-completions ]]; then
    fpath=(/usr/share/zsh-completions $fpath)
fi
autoload -Uz compinit && compinit
if command -v jt >/dev/null 2>&1; then
    source <(command jt completions zsh 2>/dev/null)
fi
zstyle ':completion:*' matcher-list 'm:{a-zA-Z}={A-Za-z}' 'r:|=*' 'l:|=*'

HISTSIZE=50000
SAVEHIST=50000
HISTFILE=~/.zsh_history
setopt EXTENDED_HISTORY HIST_EXPIRE_DUPS_FIRST HIST_IGNORE_DUPS HIST_IGNORE_SPACE
setopt SHARE_HISTORY INC_APPEND_HISTORY

autoload -U up-line-or-beginning-search down-line-or-beginning-search
zle -N up-line-or-beginning-search
zle -N down-line-or-beginning-search
bindkey '^[[A' up-line-or-beginning-search
bindkey '^[[B' down-line-or-beginning-search

if command -v fzf >/dev/null 2>&1; then
    eval "$(fzf --zsh 2>/dev/null)"
    export FZF_DEFAULT_OPTS='--height 40% --layout=reverse --border'
    if command -v fd >/dev/null 2>&1; then
        export FZF_DEFAULT_COMMAND='fd --type f --hidden --follow --exclude .git'
        export FZF_CTRL_T_COMMAND="$FZF_DEFAULT_COMMAND"
        export FZF_ALT_C_COMMAND='fd --type d --hidden --follow --exclude .git'
    elif command -v fdfind >/dev/null 2>&1; then
        export FZF_DEFAULT_COMMAND='fdfind --type f --hidden --follow --exclude .git'
        export FZF_CTRL_T_COMMAND="$FZF_DEFAULT_COMMAND"
        export FZF_ALT_C_COMMAND='fdfind --type d --hidden --follow --exclude .git'
    fi
fi

if command -v zoxide >/dev/null 2>&1; then
    eval "$(zoxide init zsh)"
fi

function set-ssh-key() {
    local key="$HOME/.ssh/$1"
    if [[ ! -f "$key" ]]; then
        echo "Key not found: $key" >&2
        echo "Available keys:" >&2
        ls ~/.ssh/*.pub 2>/dev/null | sed 's/.*\//  /; s/\.pub$//' >&2
        return 1
    fi
    ssh-add -D 2>/dev/null
    ssh-add "$key"
    echo "Active SSH key: $1"
}

alias ls='eza --icons --group-directories-first'
alias ll='eza -la --icons --group-directories-first'
alias lt='eza --tree --icons --level=2'
if command -v bat >/dev/null 2>&1; then
    alias cat='bat'
elif command -v batcat >/dev/null 2>&1; then
    alias cat='batcat'
fi
if command -v fd >/dev/null 2>&1; then
    alias find='fd'
elif command -v fdfind >/dev/null 2>&1; then
    alias find='fdfind'
fi
alias grep='rg'
alias top='btop'
alias lg='lazygit'

if [[ -n "$JT_BREW_PREFIX" && -f "$JT_BREW_PREFIX/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh" ]]; then
    source "$JT_BREW_PREFIX/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh"
elif [[ -f /usr/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh ]]; then
    source /usr/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh
fi
unset JT_BREW_PREFIX
