//! Tiny blocking HTTP helper around `ureq`.
//!
//! The CLI is a one-shot process, so a synchronous request is the simplest, lightest thing
//! that works — no async runtime, no `reqwest`. This wraps `ureq` so providers get one
//! `get_json` call that already maps transport and HTTP-status failures onto [`UsageError`].

use std::time::Duration;

use agent_usage_core::UsageError;

/// Maximum `Retry-After` we will report back (seconds); guards against absurd server values.
const MAX_RETRY_AFTER_SECS: u64 = 300;

/// A single header to send with the request.
pub type Header<'a> = (&'a str, &'a str);

/// Perform a GET and return the response body as a string.
///
/// Maps non-2xx responses onto the relevant [`UsageError`]: 401 → `Unauthorized`,
/// 429 → `RateLimited` (honoring `Retry-After`), any other non-2xx → `UnexpectedStatus`,
/// and transport errors → `Network`.
pub fn get(url: &str, headers: &[Header<'_>], timeout: Duration) -> Result<String, UsageError> {
    let agent = ureq::AgentBuilder::new()
        .timeout(timeout)
        .build();

    let mut req = agent.get(url);
    for (name, value) in headers {
        req = req.set(name, value);
    }

    match req.call() {
        Ok(resp) => resp
            .into_string()
            .map_err(|e| UsageError::Network(e.to_string())),
        Err(ureq::Error::Status(code, resp)) => Err(match code {
            401 => UsageError::Unauthorized,
            429 => {
                let retry = resp
                    .header("retry-after")
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0)
                    .min(MAX_RETRY_AFTER_SECS);
                UsageError::RateLimited(retry)
            }
            other => UsageError::UnexpectedStatus(other),
        }),
        Err(e) => Err(UsageError::Network(e.to_string())),
    }
}
