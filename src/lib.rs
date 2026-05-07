pub mod completion;
pub mod config;
pub mod dirs;
pub mod error;
pub mod exec;
pub mod expand;
pub mod highlight;
pub mod interactive;
pub mod parser;
pub mod prompt;
pub mod shell;
pub mod terminal;

pub use shell::{RunOutcome, Shell};
