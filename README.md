# plush

Soft comfy bash-compatible shell

## Install

```sh
brew install kdrag0n/tap/plush
```

## Features

- Bash-compatible command syntax
    - Pipelines, lists, conditionals, heredocs, here strings, and redirections
    - Correct ordered redirection semantics, including arbitrary file descriptors
- Fast interactive startup
    - Lightweight config loading
    - No background prompt timers
- Friendly interactive editing
    - Fish-style completions
    - Syntax highlighting
    - Pure-style prompt with Git status
- Practical shell ergonomics
    - Curated Git aliases
    - Directory-local `.env` loading without executing it
    - Background job tracking
- Compatibility path for complex Bash constructs
    - Keeps everyday commands native and falls back when Bash is the right tool

## Usage

Start an interactive shell:

```sh
plush
```

Run a command:

```sh
plush -c 'echo hello'
```

Validate Bash syntax:

```sh
plush --validate 'if true; then echo ok; fi'
```
