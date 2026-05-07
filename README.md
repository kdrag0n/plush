# plush

`plush` is a fast, bash-ish interactive shell written in Rust. It aims for a
zsh/Pure daily-driver feel with fish-inspired completion menus, while keeping
the execution core synchronous and Unix-native.

## Current Features

- Pure-style two-line prompt with cwd, git branch/status, ssh, venv/conda,
  previous status, and long command duration.
- Bash syntax validation through `brush-parser`.
- Tree-sitter Bash highlighting for normal input, with a large-input guard.
- Bracketed paste enabled; pasted regions are highlighted with adaptive gray
  background when raw paste markers reach the highlighter.
- Large pasted lines are guarded in highlighting, completion, validation, and
  execution so accidental megabyte input reports cleanly instead of wedging the
  editor or tripping OS argument limits.
- Reedline editor with Emacs bindings, history, autosuggest, and columnar
  completion menu.
- Native execution for simple commands, assignments, aliases, pipelines,
  redirections, `&&`, `||`, `;`, and `&`.
- Bash compatibility fallback for supported Bash compound syntax not yet
  lowered to the native executor; fallback commands still run in the foreground
  job-control path.
- Builtins: `cd`, `pwd`, `exit`, `export`, `unset`, `alias`, `source`, `jobs`,
  `fg`, `bg`, `disown`, `mkc`, `z`, `wttr`, `notify`, `kp`, `skp`, `ks`, `sks`,
  `fp`, and `su-user`.
- Native completions for commands, paths, environment variables, ssh hosts,
  `cd`, and common `git` flows, with on-demand bash/zsh bridge fallback.
- Terminal repair escapes after foreground programs exit.
- Directory frecency database for `z`.
- Lightweight autoenv-style `.env` and `.plushenv` loading on `cd`.
- PTY smoke coverage for background jobs, Ctrl-Z stopped jobs, stopped
  foreground pipelines, stopped Bash fallback jobs, `jobs`, and `bg`.

## Validation

macOS:

```sh
cargo test
cargo build --release
tests/pty_smoke.exp target/release/plush
target/release/plush -c 'printf hi | wc -c'
target/release/plush -c 'if true; then echo mac-ok; fi'
```

Linux in OrbStack:

```sh
orb bash -lc 'cd /Users/dragon/code/projects/plush && cargo test'
orb bash -lc 'cd /Users/dragon/code/projects/plush && CARGO_TARGET_DIR=target/orb cargo build --release'
orb bash -lc 'cd /Users/dragon/code/projects/plush && target/orb/release/plush -c "printf hi | wc -c"'
orb bash -lc 'cd /Users/dragon/code/projects/plush && target/orb/release/plush -c "if true; then echo linux-ok; fi"'
```

## Known Gaps

- Native execution for compound Bash forms is intentionally incomplete; those
  forms currently go through `/bin/bash`.
- Programmable bash/zsh completion support is on-demand and partial. Native
  completions cover the common interactive cases first.
- Terminal mode restoration after unusual full-screen program crashes needs a
  larger corpus than the current repair-escape smoke coverage.
