# jt cli bootstrap: Fish config
fish_add_path $HOME/.local/bin
if test -d /opt/homebrew
    fish_add_path /opt/homebrew/bin
else if test -d /usr/local/Homebrew
    fish_add_path /usr/local/bin
else if test -d /home/linuxbrew/.linuxbrew
    fish_add_path /home/linuxbrew/.linuxbrew/bin
end
if status is-interactive; and command -q jt
    command jt completions fish 2>/dev/null | source
end

set -gx STARSHIP_CONFIG "$HOME/.config/jt-cli/starship.toml"
if command -q starship
    starship init fish | source
end

if command -q zoxide
    zoxide init fish | source
end

if command -q fzf
    fzf --fish 2>/dev/null | source
    set -gx FZF_DEFAULT_OPTS '--height 40% --layout=reverse --border'
    if command -q fd
        set -gx FZF_DEFAULT_COMMAND 'fd --type f --hidden --follow --exclude .git'
        set -gx FZF_CTRL_T_COMMAND $FZF_DEFAULT_COMMAND
        set -gx FZF_ALT_C_COMMAND 'fd --type d --hidden --follow --exclude .git'
    else if command -q fdfind
        set -gx FZF_DEFAULT_COMMAND 'fdfind --type f --hidden --follow --exclude .git'
        set -gx FZF_CTRL_T_COMMAND $FZF_DEFAULT_COMMAND
        set -gx FZF_ALT_C_COMMAND 'fdfind --type d --hidden --follow --exclude .git'
    end
end

if status is-interactive
    abbr -a ls "eza --icons --group-directories-first"
    abbr -a ll "eza -la --icons --group-directories-first"
    abbr -a lt "eza --tree --icons --level=2"
    if command -q bat
        abbr -a cat "bat"
    else if command -q batcat
        abbr -a cat "batcat"
    end
    if command -q fd
        abbr -a find "fd"
    else if command -q fdfind
        abbr -a find "fdfind"
    end
    abbr -a grep "rg"
    abbr -a top "btop"
    abbr -a lg "lazygit"
    abbr -a cd "z"
end

function set-ssh-key
    set -l key "$HOME/.ssh/$argv[1]"
    if not test -f "$key"
        echo "Key not found: $key" >&2
        echo "Available keys:" >&2
        for file in ~/.ssh/*.pub
            echo "  "(basename $file .pub) >&2
        end
        return 1
    end
    ssh-add -D 2>/dev/null
    ssh-add "$key"
    echo "Active SSH key: $argv[1]"
end
