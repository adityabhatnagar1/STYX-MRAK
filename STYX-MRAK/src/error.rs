//! Typed errors. Nothing in the bot panics on a Discord, HTTP or database failure.

use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    /// Malformed or out-of-range user input. The message is safe to show to the user.
    #[error("{0}")]
    Validation(String),
    /// The actor is not allowed to perform the action. Safe to show.
    #[error("{0}")]
    Forbidden(String),
    /// A record does not exist. Safe to show.
    #[error("{0}")]
    NotFound(String),
    /// The action conflicts with current state (duplicate, invalid transition). Safe to show.
    #[error("{0}")]
    Conflict(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("database error: {0}")]
    Db(Box<sqlx::Error>),
    #[error("migration error: {0}")]
    Migrate(Box<sqlx::migrate::MigrateError>),
    #[error("discord error: {0}")]
    Discord(Box<serenity::Error>),
    #[error("feed error: {0}")]
    Feed(#[from] crate::feeds::source::FeedError),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

// Large foreign errors are boxed so `Result<T, Error>` stays small.
impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(Box::new(e))
    }
}
impl From<serenity::Error> for Error {
    fn from(e: serenity::Error) -> Self {
        Self::Discord(Box::new(e))
    }
}
impl From<sqlx::migrate::MigrateError> for Error {
    fn from(e: sqlx::migrate::MigrateError) -> Self {
        Self::Migrate(Box::new(e))
    }
}

impl Error {
    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }

    /// Text that is safe to show to a Discord user. Internal errors are
    /// deliberately generic so no internals or secrets leak into chat.
    pub fn user_message(&self) -> String {
        match self {
            Self::Validation(m) | Self::Forbidden(m) | Self::NotFound(m) | Self::Conflict(m) => m.clone(),
            Self::Discord(_) => {
                "Discord rejected part of that request. Check the bot's permissions and role position.".into()
            }
            _ => "Internal error. It has been logged.".into(),
        }
    }
}
