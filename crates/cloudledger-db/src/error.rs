pub type RepoResult<T> = Result<T, RepositoryError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryError {
    Backend(String),
}

impl RepositoryError {
    pub fn backend(message: impl Into<String>) -> Self {
        Self::Backend(message.into())
    }
}

impl std::fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepositoryError::Backend(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for RepositoryError {}

#[cfg(feature = "sqlite")]
impl From<rusqlite::Error> for RepositoryError {
    fn from(value: rusqlite::Error) -> Self {
        RepositoryError::backend(value.to_string())
    }
}

#[cfg(feature = "sqlite")]
impl From<serde_json::Error> for RepositoryError {
    fn from(value: serde_json::Error) -> Self {
        RepositoryError::backend(value.to_string())
    }
}
