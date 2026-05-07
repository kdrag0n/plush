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

fn default_autoenv() -> bool {
    true
}

fn default_aliases() -> BTreeMap<String, String> {
    [
        (".", "source"),
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
        ("grep", "grep --color=auto"),
        ("diff", "diff --color=auto"),
        ("ip", "ip --color=auto"),
        (
            "yay",
            "yay --norebuild --noeditmenu --nodiffmenu --askremovemake",
        ),
        ("aur", "yay -a"),
        ("se", "sudoedit"),
        ("sc", "sudo chown -R $USER:$USER"),
        ("zreload", "source ~/.config/plush/config.toml"),
        ("bat", "bat --style=plain"),
        ("p", "git cherry-pick"),
        ("x", "git reset HEAD --hard && git cherry-pick --abort"),
        ("c", "git add . && git cherry-pick --continue"),
        ("r", "git revert"),
        ("glo", "git log --oneline"),
        ("gwsf", "git status"),
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
