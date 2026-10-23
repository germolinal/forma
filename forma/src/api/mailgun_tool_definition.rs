use agent::tool::mailgun_tool::{MailgunLocation, MailgunTool};
use derive::FormaDocs;
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::ObjectDefinition;

/// Sends an email using the Mailgun API.
/// Requires the following Env variable
/// - MAILGUN_API_KEY
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
pub struct MailgunToolDefinition {
    /// Either 'eu' or 'us'
    pub location: MailgunLocation,
    /// The name and email of the sender
    /// Can be something like 'Bob <bob@my_company.com>'
    pub from: String,
    /// The domain (eg., my_company.com).
    /// Must match the one in the email set in
    /// `from`
    pub domain: String,
}

impl ObjectDefinition for MailgunToolDefinition {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

impl TryFrom<MailgunToolDefinition> for MailgunTool {
    type Error = FormaError;

    fn try_from(value: MailgunToolDefinition) -> Result<Self, Self::Error> {
        Ok(MailgunTool {
            location: value.location,
            domain: value.domain,
            from: value.from,
        })
    }
}
