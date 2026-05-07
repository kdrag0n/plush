use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_history_size")]
    pub history_size: usize,
    #[serde(default = "default_max_interactive_parse_bytes")]
    pub max_interactive_parse_bytes: usize,
    #[serde(default = "default_max_command_bytes")]
    pub max_command_bytes: usize,
    #[serde(default = "default_autoenv")]
    pub autoenv: bool,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            history_size: default_history_size(),
            max_interactive_parse_bytes: default_max_interactive_parse_bytes(),
            max_command_bytes: default_max_command_bytes(),
            autoenv: default_autoenv(),
            aliases: default_aliases(),
        }
    }
}

pub fn load() -> Config {
    let mut config = Config::default();
    if let Some(path) = config_path() {
        if let Ok(contents) = fs::read_to_string(path) {
            match toml::from_str::<Config>(&contents) {
                Ok(user_config) => {
                    config.history_size = user_config.history_size;
                    config.max_interactive_parse_bytes = user_config.max_interactive_parse_bytes;
                    config.max_command_bytes = user_config.max_command_bytes;
                    config.autoenv = user_config.autoenv;
                    config.aliases.extend(user_config.aliases);
                }
                Err(err) => eprintln!("plush: config error: {err}"),
            }
        }
    }
    install_conditional_aliases(&mut config.aliases);
    config
}

pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("plush").join("config.toml"))
}

pub fn history_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("plush")
        .join("history")
}

fn default_history_size() -> usize {
    1500
}

fn default_max_interactive_parse_bytes() -> usize {
    128 * 1024
}

fn default_max_command_bytes() -> usize {
    256 * 1024
}

fn default_autoenv() -> bool {
    true
}

fn default_aliases() -> BTreeMap<String, String> {
    [
        (".", "source"),
        ("_", "sudo"),
        ("dot", "git --git-dir=$HOME/.dot --work-tree=$HOME"),
        ("s", "systemctl"),
        ("ss", "sudo systemctl"),
        ("reboot", "sudo systemctl reboot"),
        ("shutdown", "sudo systemctl poweroff"),
        ("suspend", "sudo systemctl suspend"),
        ("hibernate", "sudo systemctl hibernate"),
        ("j", "journalctl"),
        ("sj", "sudo journalctl"),
        ("umount", "sudo umount"),
        ("mount", "sudo mount"),
        ("df", "df -kh"),
        ("du", "du -kh"),
        ("grep", "grep --color=auto"),
        ("diff", "diff --color=auto"),
        ("diffu", "diff --unified"),
        ("ip", "ip --color=auto"),
        (
            "yay",
            "yay --norebuild --noeditmenu --nodiffmenu --askremovemake",
        ),
        ("e", "$EDITOR"),
        ("aur", "yay -a"),
        ("se", "sudoedit"),
        ("sc", "sudo chown -R $USER:$USER"),
        ("sa", "alias | grep -i"),
        ("zreload", "reload"),
        ("bat", "bat --style=plain"),
        (
            "get",
            "curl --continue-at - --location --progress-bar --remote-name --remote-time",
        ),
        ("http-serve", "python3 -m http.server"),
        ("o", "open"),
        ("po", "popd"),
        ("pbc", "pbcopy"),
        ("pbp", "pbpaste"),
        ("pu", "pushd"),
        ("topc", "top -o cpu"),
        ("topm", "top -o vsize"),
        ("g", "git"),
        ("p", "git cherry-pick"),
        ("x", "git reset HEAD --hard && git cherry-pick --abort"),
        ("c", "git add . && git cherry-pick --continue"),
        ("r", "git revert"),
        ("gCl", "git --no-pager diff --name-only --diff-filter=U"),
        ("gCo", "git checkout --ours --"),
        ("gCt", "git checkout --theirs --"),
        ("gR", "git remote"),
        ("gRa", "git remote add"),
        ("gRl", "git remote --verbose"),
        ("gRm", "git remote rename"),
        ("gRp", "git remote prune"),
        ("gRs", "git remote show"),
        ("gRu", "git remote update"),
        ("gRx", "git remote rm"),
        ("gS", "git submodule"),
        ("gSI", "git submodule update --init --recursive"),
        ("gSa", "git submodule add"),
        ("gSf", "git submodule foreach"),
        ("gSi", "git submodule init"),
        ("gSl", "git submodule status"),
        ("gSs", "git submodule sync"),
        ("gSu", "git submodule update --remote --recursive"),
        ("gb", "git branch"),
        ("gbD", "git branch --delete --force"),
        ("gba", "git branch --all --verbose"),
        ("gbc", "git checkout -b"),
        ("gbd", "git branch --delete"),
        ("gbl", "git branch --verbose"),
        ("gbm", "git branch --move"),
        ("gc", "git commit --verbose"),
        ("gcF", "git commit --verbose --amend"),
        ("gcO", "git checkout --patch"),
        ("gcP", "git cherry-pick --no-commit"),
        ("gcR", "git reset HEAD^"),
        ("gca", "git commit --verbose --all"),
        ("gcam", "git commit --all --message"),
        ("gcf", "git commit --amend --reuse-message HEAD"),
        ("gcm", "git commit --message"),
        ("gco", "git checkout"),
        ("gcp", "git cherry-pick --ff"),
        ("gcr", "git revert"),
        ("gcs", "git show"),
        ("gcsS", "git show --pretty=short --show-signature"),
        ("gd", "git ls-files"),
        ("gdc", "git ls-files --cached"),
        (
            "gdi",
            "git status --porcelain --short --ignored | sed -n 's/^!! //p'",
        ),
        ("gdk", "git ls-files --killed"),
        ("gdm", "git ls-files --modified"),
        ("gdu", "git ls-files --other --exclude-standard"),
        ("gdx", "git ls-files --deleted"),
        ("gf", "git fetch"),
        ("gfa", "git fetch --all"),
        ("gfc", "git clone"),
        ("gfcr", "git clone --recurse-submodules"),
        ("gfm", "git pull"),
        ("gfma", "git pull --autostash"),
        ("gfr", "git pull --rebase"),
        ("gfra", "git pull --rebase --autostash"),
        ("gg", "git grep"),
        ("ggL", "git grep --files-without-matches"),
        ("ggi", "git grep --ignore-case"),
        ("ggl", "git grep --files-with-matches"),
        ("ggv", "git grep --invert-match"),
        ("ggw", "git grep --word-regexp"),
        ("giA", "git add --patch"),
        ("giD", "git diff --no-ext-diff --cached --word-diff"),
        ("giR", "git reset --patch"),
        ("giX", "git rm -r --force --cached"),
        ("gia", "git add"),
        ("gid", "git diff --no-ext-diff --cached"),
        ("gir", "git reset"),
        ("giu", "git add --update"),
        ("gix", "git rm -r --cached"),
        (
            "gl",
            "git log --pretty=format:%C(yellow)%h%Creset %Cgreen%cr%Creset %C(auto)%d%Creset %s",
        ),
        ("glS", "git log --show-signature"),
        ("glg", "git log --topo-order --graph --oneline --decorate"),
        ("glo", "git log --oneline"),
        ("gm", "git merge"),
        ("gmC", "git merge --no-commit"),
        ("gmF", "git merge --no-ff"),
        ("gma", "git merge --abort"),
        ("gmt", "git mergetool"),
        ("gp", "git push"),
        ("gpA", "git push --all && git push --tags"),
        ("gpF", "git push --force"),
        ("gpa", "git push --all"),
        ("gpf", "git push --force-with-lease"),
        ("gpt", "git push --tags"),
        ("gr", "git rebase"),
        ("gra", "git rebase --abort"),
        ("grc", "git rebase --continue"),
        ("gri", "git rebase --interactive"),
        ("grs", "git rebase --skip"),
        ("gs", "git stash"),
        ("gsa", "git stash apply"),
        ("gsd", "git stash show --patch --stat"),
        ("gsl", "git stash list"),
        ("gsp", "git stash pop"),
        ("gss", "git stash save --include-untracked"),
        ("gsw", "git stash save --include-untracked --keep-index"),
        ("gsx", "git stash drop"),
        ("gt", "git tag"),
        ("gtl", "git tag --list"),
        ("gts", "git tag --sign"),
        ("gtv", "git verify-tag"),
        ("gwC", "git clean --force"),
        ("gwD", "git diff --no-ext-diff --word-diff"),
        ("gwR", "git reset --hard"),
        ("gwX", "git rm -r --force"),
        ("gwc", "git clean --dry-run"),
        ("gwd", "git diff --no-ext-diff"),
        ("gwr", "git reset --soft"),
        ("gws", "git status --short"),
        ("gwsf", "git status"),
        ("gwx", "git rm -r"),
        ("gsh", "git show"),
        ("d", "kitty +kitten diff"),
        ("icat", "kitty +kitten icat"),
        ("kssh", "kitty +kitten ssh"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

fn install_conditional_aliases(aliases: &mut BTreeMap<String, String>) {
    if which::which("fdfind").is_ok() {
        aliases.insert("fd".to_string(), "fdfind".to_string());
    }
    if which::which("helix").is_ok() {
        aliases.insert("hx".to_string(), "helix".to_string());
    }
    if which::which("lsd").is_ok() {
        aliases.insert("lsd".to_string(), "lsd --icon never".to_string());
        aliases.insert("ls".to_string(), "lsd".to_string());
        aliases.insert("la".to_string(), "lsd -la".to_string());
        aliases.insert("lt".to_string(), "lsd -glT".to_string());
        aliases.insert("ll".to_string(), "lsd -l".to_string());
        aliases.insert("l".to_string(), "ll".to_string());
    }
    let pac = if which::which("pacman").is_ok() {
        "sudo pacman"
    } else if which::which("tsu").is_ok() {
        "$HOME/bin/pacapt"
    } else {
        "sudo $HOME/bin/pacapt"
    };
    aliases.insert("pac".to_string(), pac.to_string());
}
