//! The single error type every provider returns.
//!
//! It is intentionally provider-neutral: an `UsageError` describes a class of failure
//! (network, auth, parse, credentials, unsupported) without leaking any agent-specific
//! detail in its variants. Providers map their own failures onto these variants so the
//! CLI can render one stable error contract for every agent.

/// Errors any provider may surface while resolving credentials or fetching usage.
#[derive(Debug, Clone, thiserror::Error)]
pub enum UsageError {
    #[error("network error: {0}")]
    Network(String),

    #[error("failed to parse usage response: {0}")]
    Parse(String),

    #[error("unauthorized -- credentials may be expired")]
    Unauthorized,

    #[error("rate limited by source (retry after {0}s)")]
    RateLimited(u64),

    #[error("unexpected HTTP status: {0}")]
    UnexpectedStatus(u16),

    #[error("cannot read credentials: {0}")]
    CredentialsRead(String),

    #[error("cannot parse credentials: {0}")]
    CredentialsParse(String),

    #[error("credentials missing required token field")]
    CredentialsMissingToken,

    /// The provider exists but cannot yet report usage (e.g. a stub provider, or a
    /// usage source that has not been wired up). Carries a human-readable reason.
    #[error("{0}")]
    Unsupported(String),
}

impl UsageError {
    /// Stable, lowercase machine-readable discriminant for the JSON error contract.
    pub fn kind(&self) -> &'static str {
        match self {
            UsageError::Network(_) => "network",
            UsageError::Parse(_) => "parse",
            UsageError::Unauthorized => "unauthorized",
            UsageError::RateLimited(_) => "rate_limited",
            UsageError::UnexpectedStatus(_) => "unexpected_status",
            UsageError::CredentialsRead(_) => "credentials_read",
            UsageError::CredentialsParse(_) => "credentials_parse",
            UsageError::CredentialsMissingToken => "credentials_missing_token",
            UsageError::Unsupported(_) => "unsupported",
        }
    }
}
