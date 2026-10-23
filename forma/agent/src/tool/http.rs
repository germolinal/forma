//! The HTTP client tools reach other services with.

use std::sync::LazyLock;

/// Shared by every call a tool makes, so connections — and their TLS
/// handshakes — are kept and reused from one call, conversation and user to the
/// next. Each request carries its own credentials: sharing a connection shares
/// nothing else.
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

/// The client to send a tool's requests with.
pub(crate) fn client() -> &'static reqwest::Client {
    &CLIENT
}
