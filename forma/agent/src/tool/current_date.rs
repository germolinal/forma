//! Tells the agent today's date and time, in the user's time zone.
//!
//! The date is not put in the system prompt on purpose: it would change the
//! prompt every day (every minute, with the time), and with it the prompt
//! cache. A tool result is appended to the conversation instead, so asking for
//! the date costs a round trip only in the replies that need it.
//!
//! Which time zone, in order:
//!
//! 1. the user's device, sent by the client in the `x-time-zone` header
//!    ([`protocols::TIME_ZONE_HEADER`]). The web app and the Apple client send
//!    it; WhatsApp, Telegram and the like cannot.
//! 2. one the agent passes in the `timezone` argument, because the user told
//!    it where they are (it is meant to remember it, e.g. in a profile).
//! 3. the agent's own `timezone`, set by its author in the agent definition.
//! 4. none: the tool says so, with the date in UTC, and asks the agent to ask
//!    the user where they are and call again with `timezone`.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use genai_client::{JsonSchema, schema::SchemaType};
use protocols::{FormaDB, FormaError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{CallContext, ToolTrait, tool::mailgun_tool::string_schema};

/// Parses an IANA time zone name, such as `America/Santiago`.
pub fn parse_timezone(name: &str) -> Result<Tz, FormaError> {
    name.trim().parse::<Tz>().map_err(|_| {
        FormaError::Input(format!(
            "'{name}' is not a time zone: use an IANA name such as 'America/Santiago' or 'Europe/Madrid'"
        ))
    })
}

/// The tool. It has no configuration of its own: the agent's default zone
/// travels in the call context, like the user's.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CurrentDateTool {}

#[derive(Debug, Default, Deserialize)]
struct CurrentDateInput {
    #[serde(default)]
    timezone: Option<String>,
}

/// Where the zone came from, as the agent is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Device,
    User,
    Agent,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Device => "the user's device",
            Source::User => "what the user said",
            Source::Agent => "the agent's default",
        }
    }
}

/// Picks the zone, following the order in the module docs. An argument that
/// is not a zone is an error the agent can fix; a header that is not one is
/// ignored, since the agent can do nothing about it.
fn resolve(
    device: Option<&str>,
    argument: Option<&str>,
    agent: Option<&str>,
) -> Result<Option<(Tz, Source)>, FormaError> {
    if let Some(tz) = device.and_then(|d| parse_timezone(d).ok()) {
        return Ok(Some((tz, Source::Device)));
    }
    if let Some(a) = argument.map(str::trim).filter(|a| !a.is_empty()) {
        return Ok(Some((parse_timezone(a)?, Source::User)));
    }
    if let Some(tz) = agent.and_then(|a| parse_timezone(a).ok()) {
        return Ok(Some((tz, Source::Agent)));
    }
    Ok(None)
}

/// The answer, for a given instant: kept apart from the clock so tests can
/// pin it.
fn answer(now: DateTime<Utc>, zone: Option<(Tz, Source)>) -> Value {
    match zone {
        Some((tz, source)) => {
            let local = now.with_timezone(&tz);
            json!({
                "date": local.format("%Y-%m-%d").to_string(),
                "weekday": local.format("%A").to_string(),
                "time": local.format("%H:%M").to_string(),
                "timezone": tz.name(),
                "timezone_source": source.as_str(),
            })
        }
        None => json!({
            "timezone_known": false,
            "utc_date": now.format("%Y-%m-%d").to_string(),
            "utc_time": now.format("%H:%M").to_string(),
            "instructions": "The user's time zone is unknown, so their date may differ from \
                the UTC date. Ask the user which city or country they are in, then call this \
                tool again with `timezone` (an IANA name such as 'America/Santiago'). \
                Remember it for next time.",
        }),
    }
}

impl TryFrom<CurrentDateTool> for JsonSchema {
    type Error = FormaError;

    fn try_from(_item: CurrentDateTool) -> Result<JsonSchema, FormaError> {
        let mut props = HashMap::new();
        props.insert(
            "timezone".to_string(),
            string_schema(
                "Optional. The user's time zone as an IANA name (e.g. 'America/Santiago'), \
                 only if the user told you where they are. Leave it out otherwise.",
            ),
        );
        Ok(JsonSchema {
            schema_type: Some(SchemaType::Object),
            properties: Some(props),
            required: Some(vec![]),
            additional_properties: false,
            ..Default::default()
        })
    }
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for CurrentDateTool {
    fn check(&self) -> Result<(), FormaError> {
        Ok(())
    }

    async fn execute(
        &self,
        input: Value,
        ctx: CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        let input: CurrentDateInput = if input.is_null() {
            CurrentDateInput::default()
        } else {
            serde_json::from_value(input)
                .map_err(|e| FormaError::Input(format!("current date: invalid input: {e}")))?
        };
        let zone = resolve(
            ctx.user_timezone.as_deref(),
            input.timezone.as_deref(),
            ctx.agent_timezone.as_deref(),
        )?;
        Ok(Some(answer(Utc::now(), zone)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// 03:30 UTC on a Friday is still Thursday evening in Santiago.
    fn friday_early_utc() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 16, 3, 30, 0).unwrap()
    }

    #[test]
    fn the_device_zone_wins() {
        let zone = resolve(Some("America/Santiago"), Some("Europe/Madrid"), Some("UTC")).unwrap();
        let a = answer(friday_early_utc(), zone);
        assert_eq!(a["date"], "2026-10-16");
        assert_eq!(a["timezone"], "America/Santiago");
        assert_eq!(a["timezone_source"], "the user's device");
    }

    #[test]
    fn the_date_is_the_user_date_not_the_server_date() {
        let zone = resolve(Some("America/Santiago"), None, None).unwrap();
        let a = answer(friday_early_utc(), zone);
        // Santiago is UTC-3 in October (summer time): 00:30 on Friday the 16th.
        assert_eq!(a["date"], "2026-10-16");
        assert_eq!(a["time"], "00:30");
        assert_eq!(a["weekday"], "Friday");

        let zone = resolve(Some("America/Los_Angeles"), None, None).unwrap();
        let a = answer(friday_early_utc(), zone);
        // Los Angeles is UTC-7 in October: Thursday the 15th, 20:30.
        assert_eq!(a["date"], "2026-10-15");
        assert_eq!(a["weekday"], "Thursday");
    }

    #[test]
    fn a_bad_header_is_ignored_and_the_argument_used() {
        let zone = resolve(Some("not/a-zone"), Some("Europe/Madrid"), None).unwrap();
        let (tz, source) = zone.unwrap();
        assert_eq!(tz.name(), "Europe/Madrid");
        assert_eq!(source, Source::User);
    }

    #[test]
    fn the_argument_beats_the_agent_default() {
        let zone = resolve(None, Some("Europe/Madrid"), Some("America/Santiago")).unwrap();
        assert_eq!(zone.unwrap().1, Source::User);
    }

    #[test]
    fn a_bad_argument_is_an_error_the_agent_can_fix() {
        let err = resolve(None, Some("Santiago"), Some("America/Santiago")).unwrap_err();
        assert!(err.to_string().contains("IANA"), "{err}");
    }

    #[test]
    fn the_agent_default_is_the_last_resort() {
        let zone = resolve(None, None, Some("America/Santiago")).unwrap();
        assert_eq!(zone.unwrap().1, Source::Agent);
    }

    #[test]
    fn with_no_zone_it_asks_the_agent_to_ask() {
        let zone = resolve(None, Some("  "), None).unwrap();
        let a = answer(friday_early_utc(), zone);
        assert_eq!(a["timezone_known"], false);
        assert_eq!(a["utc_date"], "2026-10-16");
        assert!(a["instructions"].as_str().unwrap().contains("Ask the user"));
    }
}
