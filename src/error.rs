use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlushError {
    #[error("{0}")]
    Message(String),
    #[error("syntax error: {0}")]
    Syntax(String),
    #[error("unsupported syntax for now: {0}")]
    Unsupported(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Nix(#[from] nix::Error),
}

pub type Result<T> = std::result::Result<T, PlushError>;

impl PlushError {
    pub fn msg(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}
