use agent::tool::current_date::CurrentDateTool;
use derive::FormaDocs;
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::ObjectDefinition;

/// Tells the agent today's date, weekday and time in the user's time zone.
///
/// The date is deliberately not available in prompts: it would change the
/// system prompt every day and spoil the prompt cache. A tool result is added
/// to the end of the conversation instead, so the cost is one extra round trip
/// in the replies that need the date. Say in the tool's `description` (and in
/// the prompt, if it matters) when the agent should call it: before anything
/// that depends on "today", a deadline or the day of the week.
///
/// The time zone is, in order:
///
/// 1. the one the user's client sends in the `x-time-zone` header (an IANA
///    name such as `America/Santiago`). The web app and the Apple client send
///    it; WhatsApp and Telegram cannot;
/// 2. one the agent passes in the tool's optional `timezone` argument, because
///    the user said where they are (have the agent remember it, e.g. in the
///    user's profile);
/// 3. the agent's `timezone` field, in the agent definition;
/// 4. none: the tool answers with the UTC date and asks the agent to ask the
///    user where they are, and to call again with `timezone`.
///
/// ```yaml
/// - name: fecha-actual
///   description: |
///     Devuelve la fecha, el día de la semana y la hora del usuario. Úsala antes
///     de responder cualquier cosa que dependa de la fecha.
///   tool:
///     type: current-date
/// ```
#[derive(Clone, Debug, Serialize, Deserialize, Default, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct CurrentDateToolDefinition {}

impl ObjectDefinition for CurrentDateToolDefinition {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

impl TryFrom<CurrentDateToolDefinition> for CurrentDateTool {
    type Error = FormaError;

    fn try_from(_value: CurrentDateToolDefinition) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}
