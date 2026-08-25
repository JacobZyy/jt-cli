if status is-interactive
    function __terminal_setup_git_current_branch -d "Output git's current branch name"
        begin
            git symbolic-ref HEAD; or \
            git rev-parse --short HEAD; or return
        end 2>/dev/null | sed -e 's|^refs/heads/||'
    end

    function __terminal_setup_git_default_branch -d "Resolve the repository default branch"
        command git rev-parse --git-dir &>/dev/null; or return
        if set -l default_branch (command git config --get init.defaultBranch)
            and command git show-ref -q --verify refs/heads/{$default_branch}
            echo $default_branch
        else if command git show-ref -q --verify refs/heads/main
            echo main
        else
            echo master
        end
    end

    function __terminal_setup_git_create_abbr -d "Create a git abbreviation"
        set -l name $argv[1]
        set -l body $argv[2..-1]

        if contains -- $name $__terminal_setup_git_abbreviations
            abbr --erase $name 2>/dev/null
        end

        set -ga __terminal_setup_git_abbreviations $name
        abbr -a -g $name $body
    end

    if set -q __terminal_setup_git_abbreviations[1]
        for ab in $__terminal_setup_git_abbreviations
            abbr --erase $ab 2>/dev/null
        end
    end
    set -e __terminal_setup_git_abbreviations

    __terminal_setup_git_create_abbr g          git
    __terminal_setup_git_create_abbr ga         git add
    __terminal_setup_git_create_abbr gaa        git add --all
    __terminal_setup_git_create_abbr gau        git add --update
    __terminal_setup_git_create_abbr gapa       git add --patch
    __terminal_setup_git_create_abbr gap        git apply
    __terminal_setup_git_create_abbr gb         git branch -vv
    __terminal_setup_git_create_abbr gba        git branch -a -v
    __terminal_setup_git_create_abbr gban       git branch -a -v --no-merged
    __terminal_setup_git_create_abbr gbd        git branch -d
    __terminal_setup_git_create_abbr gbD        git branch -D
    __terminal_setup_git_create_abbr ggsup      git branch --set-upstream-to=origin/\(__terminal_setup_git_current_branch\)
    __terminal_setup_git_create_abbr gbl        git blame -b -w
    __terminal_setup_git_create_abbr gbs        git bisect
    __terminal_setup_git_create_abbr gbsb       git bisect bad
    __terminal_setup_git_create_abbr gbsg       git bisect good
    __terminal_setup_git_create_abbr gbsr       git bisect reset
    __terminal_setup_git_create_abbr gbss       git bisect start
    __terminal_setup_git_create_abbr gc         git commit -v
    __terminal_setup_git_create_abbr gc!        git commit -v --amend
    __terminal_setup_git_create_abbr gcn!       git commit -v --no-edit --amend
    __terminal_setup_git_create_abbr gca        git commit -v -a
    __terminal_setup_git_create_abbr gca!       git commit -v -a --amend
    __terminal_setup_git_create_abbr gcan!      git commit -v -a --no-edit --amend
    __terminal_setup_git_create_abbr gcv        git commit -v --no-verify
    __terminal_setup_git_create_abbr gcav       git commit -a -v --no-verify
    __terminal_setup_git_create_abbr gcav!      git commit -a -v --no-verify --amend
    __terminal_setup_git_create_abbr gcm        git commit -m
    __terminal_setup_git_create_abbr gcam       git commit -a -m
    __terminal_setup_git_create_abbr gcs        git commit -S
    __terminal_setup_git_create_abbr gscam      git commit -S -a -m
    __terminal_setup_git_create_abbr gcfx       git commit --fixup
    __terminal_setup_git_create_abbr gcf        git config --list
    __terminal_setup_git_create_abbr gcl        git clone
    __terminal_setup_git_create_abbr gclean     git clean -di
    __terminal_setup_git_create_abbr gclean!    git clean -dfx
    __terminal_setup_git_create_abbr gclean!!   "git reset --hard; and git clean -dfx"
    __terminal_setup_git_create_abbr gcount     git shortlog -sn
    __terminal_setup_git_create_abbr gcp        git cherry-pick
    __terminal_setup_git_create_abbr gcpa       git cherry-pick --abort
    __terminal_setup_git_create_abbr gcpc       git cherry-pick --continue
    __terminal_setup_git_create_abbr gd         git diff
    __terminal_setup_git_create_abbr gdca       git diff --cached
    __terminal_setup_git_create_abbr gds        git diff --stat
    __terminal_setup_git_create_abbr gdsc       git diff --stat --cached
    __terminal_setup_git_create_abbr gdt        git diff-tree --no-commit-id --name-only -r
    __terminal_setup_git_create_abbr gdw        git diff --word-diff
    __terminal_setup_git_create_abbr gdwc       git diff --word-diff --cached
    __terminal_setup_git_create_abbr gdto       git difftool
    __terminal_setup_git_create_abbr gdg        git diff --no-ext-diff
    __terminal_setup_git_create_abbr gignore    git update-index --assume-unchanged
    __terminal_setup_git_create_abbr gf         git fetch
    __terminal_setup_git_create_abbr gfa        git fetch --all --prune
    __terminal_setup_git_create_abbr gfm        "git fetch origin (__terminal_setup_git_default_branch) --prune; and git merge FETCH_HEAD"
    __terminal_setup_git_create_abbr gfo        git fetch origin
    __terminal_setup_git_create_abbr gl         git pull
    __terminal_setup_git_create_abbr ggl        git pull origin \(__terminal_setup_git_current_branch\)
    __terminal_setup_git_create_abbr gll        git pull origin
    __terminal_setup_git_create_abbr glr        git pull --rebase
    __terminal_setup_git_create_abbr glg        git log --stat
    __terminal_setup_git_create_abbr glgg       git log --graph
    __terminal_setup_git_create_abbr glgga      git log --graph --decorate --all
    __terminal_setup_git_create_abbr glo        git log --oneline --decorate --color
    __terminal_setup_git_create_abbr glog       git log --oneline --decorate --color --graph
    __terminal_setup_git_create_abbr gloga      git log --oneline --decorate --color --graph --all
    __terminal_setup_git_create_abbr glom       git log --oneline --decorate --color \(__terminal_setup_git_default_branch\)..
    __terminal_setup_git_create_abbr glod       git log --oneline --decorate --color develop..
    __terminal_setup_git_create_abbr gloo       "git log --pretty=format:'%C(yellow)%h %Cred%ad %Cblue%an%Cgreen%d %Creset%s' --date=short"
    __terminal_setup_git_create_abbr gm         git merge
    __terminal_setup_git_create_abbr gma        git merge --abort
    __terminal_setup_git_create_abbr gmt        git mergetool --no-prompt
    __terminal_setup_git_create_abbr gmom       git merge origin/\(__terminal_setup_git_default_branch\)
    __terminal_setup_git_create_abbr gp         git push
    __terminal_setup_git_create_abbr gp!        git push --force-with-lease
    __terminal_setup_git_create_abbr gpo        git push origin
    __terminal_setup_git_create_abbr gpo!       git push --force-with-lease origin
    __terminal_setup_git_create_abbr gpv        git push --no-verify
    __terminal_setup_git_create_abbr gpv!       git push --no-verify --force-with-lease
    __terminal_setup_git_create_abbr ggp        git push origin \(__terminal_setup_git_current_branch\)
    __terminal_setup_git_create_abbr ggp!       git push origin \(__terminal_setup_git_current_branch\) --force-with-lease
    __terminal_setup_git_create_abbr gpu        git push origin \(__terminal_setup_git_current_branch\) --set-upstream
    __terminal_setup_git_create_abbr gpoat      "git push origin --all; and git push origin --tags"
    __terminal_setup_git_create_abbr ggpnp      "git pull origin (__terminal_setup_git_current_branch); and git push origin (__terminal_setup_git_current_branch)"
    __terminal_setup_git_create_abbr gr         git remote -vv
    __terminal_setup_git_create_abbr gra        git remote add
    __terminal_setup_git_create_abbr grb        git rebase
    __terminal_setup_git_create_abbr grba       git rebase --abort
    __terminal_setup_git_create_abbr grbc       git rebase --continue
    __terminal_setup_git_create_abbr grbi       git rebase --interactive
    __terminal_setup_git_create_abbr grbm       git rebase \(__terminal_setup_git_default_branch\)
    __terminal_setup_git_create_abbr grbmi      git rebase \(__terminal_setup_git_default_branch\) --interactive
    __terminal_setup_git_create_abbr grbmia     git rebase \(__terminal_setup_git_default_branch\) --interactive --autosquash
    __terminal_setup_git_create_abbr grbom      "git fetch origin (__terminal_setup_git_default_branch); and git rebase FETCH_HEAD"
    __terminal_setup_git_create_abbr grbomi     "git fetch origin (__terminal_setup_git_default_branch); and git rebase FETCH_HEAD --interactive"
    __terminal_setup_git_create_abbr grbomia    "git fetch origin (__terminal_setup_git_default_branch); and git rebase FETCH_HEAD --interactive --autosquash"
    __terminal_setup_git_create_abbr grbd       git rebase develop
    __terminal_setup_git_create_abbr grbdi      git rebase develop --interactive
    __terminal_setup_git_create_abbr grbdia     git rebase develop --interactive --autosquash
    __terminal_setup_git_create_abbr grbs       git rebase --skip
    __terminal_setup_git_create_abbr ggu        git pull --rebase origin \(__terminal_setup_git_current_branch\)
    __terminal_setup_git_create_abbr grev       git revert
    __terminal_setup_git_create_abbr grh        git reset
    __terminal_setup_git_create_abbr grhh       git reset --hard
    __terminal_setup_git_create_abbr grhpa      git reset --patch
    __terminal_setup_git_create_abbr grm        git rm
    __terminal_setup_git_create_abbr grmc       git rm --cached
    __terminal_setup_git_create_abbr grmv       git remote rename
    __terminal_setup_git_create_abbr grpo       git remote prune origin
    __terminal_setup_git_create_abbr grrm       git remote remove
    __terminal_setup_git_create_abbr grs        git restore
    __terminal_setup_git_create_abbr grset      git remote set-url
    __terminal_setup_git_create_abbr grss       git restore --source
    __terminal_setup_git_create_abbr grst       git restore --staged
    __terminal_setup_git_create_abbr grup       git remote update
    __terminal_setup_git_create_abbr grv        git remote -v
    __terminal_setup_git_create_abbr gsh        git show
    __terminal_setup_git_create_abbr gsd        git svn dcommit
    __terminal_setup_git_create_abbr gsr        git svn rebase
    __terminal_setup_git_create_abbr gsb        git status -sb
    __terminal_setup_git_create_abbr gss        git status -s
    __terminal_setup_git_create_abbr gst        git status
    __terminal_setup_git_create_abbr gsta       git stash
    __terminal_setup_git_create_abbr gstd       git stash drop
    __terminal_setup_git_create_abbr gstl       git stash list
    __terminal_setup_git_create_abbr gstp       git stash pop
    __terminal_setup_git_create_abbr gsts       git stash show --text
    __terminal_setup_git_create_abbr gsu        git submodule update
    __terminal_setup_git_create_abbr gsur       git submodule update --recursive
    __terminal_setup_git_create_abbr gsuri      git submodule update --recursive --init
    __terminal_setup_git_create_abbr gts        git tag -s
    __terminal_setup_git_create_abbr gtv        "git tag | sort -V"
    __terminal_setup_git_create_abbr gsw        git switch
    __terminal_setup_git_create_abbr gswc       git switch --create
    __terminal_setup_git_create_abbr gunignore  git update-index --no-assume-unchanged
    __terminal_setup_git_create_abbr gup        git pull --rebase
    __terminal_setup_git_create_abbr gupv       git pull --rebase -v
    __terminal_setup_git_create_abbr gupa       git pull --rebase --autostash
    __terminal_setup_git_create_abbr gupav      git pull --rebase --autostash -v
    __terminal_setup_git_create_abbr gwch       git log -p --abbrev-commit --pretty=medium --raw --no-merges
    __terminal_setup_git_create_abbr gco        git checkout
    __terminal_setup_git_create_abbr gcb        git checkout -b
    __terminal_setup_git_create_abbr gcod       git checkout develop
    __terminal_setup_git_create_abbr gcom       git checkout \(__terminal_setup_git_default_branch\)
    __terminal_setup_git_create_abbr gfb        git flow bugfix
    __terminal_setup_git_create_abbr gff        git flow feature
    __terminal_setup_git_create_abbr gfr        git flow release
    __terminal_setup_git_create_abbr gfh        git flow hotfix
    __terminal_setup_git_create_abbr gfs        git flow support
    __terminal_setup_git_create_abbr gfbs       git flow bugfix start
    __terminal_setup_git_create_abbr gffs       git flow feature start
    __terminal_setup_git_create_abbr gfrs       git flow release start
    __terminal_setup_git_create_abbr gfhs       git flow hotfix start
    __terminal_setup_git_create_abbr gfss       git flow support start
    __terminal_setup_git_create_abbr gfbt       git flow bugfix track
    __terminal_setup_git_create_abbr gfft       git flow feature track
    __terminal_setup_git_create_abbr gfrt       git flow release track
    __terminal_setup_git_create_abbr gfht       git flow hotfix track
    __terminal_setup_git_create_abbr gfst       git flow support track
    __terminal_setup_git_create_abbr gfp        git flow publish
    __terminal_setup_git_create_abbr gwt        git worktree
    __terminal_setup_git_create_abbr gwta       git worktree add
    __terminal_setup_git_create_abbr gwtls      git worktree list
    __terminal_setup_git_create_abbr gwtlo      git worktree lock
    __terminal_setup_git_create_abbr gwtmv      git worktree move
    __terminal_setup_git_create_abbr gwtpr      git worktree prune
    __terminal_setup_git_create_abbr gwtrm      git worktree remove
    __terminal_setup_git_create_abbr gwtulo     git worktree unlock
    __terminal_setup_git_create_abbr gmr        git push origin \(__terminal_setup_git_current_branch\) --set-upstream -o merge_request.create
    __terminal_setup_git_create_abbr gmwps      git push origin \(__terminal_setup_git_current_branch\) --set-upstream -o merge_request.create -o merge_request.merge_when_pipeline_succeeds

    function gdv -w "git diff -w" -d "Pipe git diff to view"
        git diff -w $argv | view -
    end

    function glp -d "git log at requested pretty level" -a format
        set -q format[1]; and git log --pretty=$format
    end
    complete -c glp -x -a "(complete -C 'git log --pretty=' | sed 's/^--pretty=//')"

    function gtest -d "Run a command against staged changes only"
        git stash push -q --keep-index --include-untracked; or return
        command $argv
        set -l cmdstatus $status
        git reset -q
        git restore .
        git stash pop -q --index; or return $status
        return $cmdstatus
    end

    function grt -d "cd into the top of the current repository or submodule"
        cd (git rev-parse --show-toplevel; or echo ".")
    end

    function gtl -d "List tags matching prefix" -a prefix
        git tag --sort=-v:refname -n -l $prefix\*
    end

    function gignored -w 'grep "^[[:lower:]]"' -d "List temporarily ignored files"
        git ls-files -v | grep "^[[:lower:]]" $argv
    end

    function grel -d "Print path relative to repository root"
        set -l repo_dir (git rev-parse --show-prefix)
        test -n "$repo_dir"; and echo "/$repo_dir"; or echo "/"
    end

    function gwip -d "Commit a work-in-progress snapshot"
        git add -A
        git rm (git ls-files --deleted) 2>/dev/null
        git commit -m "--wip--" --no-verify
    end

    function gunwip -d "Undo the last work-in-progress commit"
        git log -n 1 | grep -q -c "\--wip--"; and git reset HEAD~1
    end

    function grename -d "Rename a branch locally and in origin" -a old new
        if test (count $argv) -ne 2
            echo "Usage: "(status -u)" old_branch new_branch"
            return 1
        end
        git branch -m $old $new
        git push origin :$old
        and git push --set-upstream origin $new
    end
    complete -c grename -x -a "(complete -C 'git branch ')"

    function gbage -d "List local branches and display their age"
        git for-each-ref --sort=committerdate refs/heads/ \
            --format="%(HEAD) %(color:yellow)%(refname:short)%(color:reset) - %(color:red)%(objectname:short)%(color:reset) - %(contents:subject) - %(authorname) (%(color:green)%(committerdate:relative)%(color:reset))"
    end

    functions -e __terminal_setup_git_create_abbr
end
