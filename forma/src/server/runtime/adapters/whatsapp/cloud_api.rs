use std::sync::LazyLock;

use protocols::FormaError;
use reqwest::Client;

#[cfg(not(feature = "airtight"))]
use reqwest::RequestBuilder;
use serde::{Deserialize, Serialize};

use super::WhatsappAPICredentials;
use crate::server::runtime::adapters::chat::{
    APPROVAL_PROMPT, APPROVE_LABEL, Approval, ChatMedia, REJECT_LABEL, truncate_chars,
};

const API_VERSION: &str = "v25.0";

/// The client every call to the Cloud API goes through, so its connection is
/// kept and reused, instead of being opened (and its TLS handshake made) again
/// for every message sent and every media fetched.
static CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

fn messages_api_url(phone_number_id: &str) -> String {
    format!("https://graph.facebook.com/{API_VERSION}/{phone_number_id}/messages")
}

#[cfg(not(feature = "airtight"))]
fn media_api_url(phone_number_id: &str) -> String {
    format!("https://graph.facebook.com/{API_VERSION}/{phone_number_id}/media")
}

#[cfg(not(feature = "airtight"))]
fn media_lookup_url(media_id: &str) -> String {
    format!("https://graph.facebook.com/{API_VERSION}/{media_id}")
}

/// What the Graph API answers when asked about a media handle: a short-lived
/// URL that still needs the access token to actually fetch.
#[cfg(not(feature = "airtight"))]
#[derive(Debug, Deserialize)]
struct MediaHandle {
    url: String,
    #[serde(default)]
    mime_type: String,
}

/// What `POST /{phone-number-id}/media` answers: the handle a message can
/// then refer to.
#[cfg(not(feature = "airtight"))]
#[derive(Debug, Deserialize)]
struct UploadedMedia {
    id: String,
}

/// Turns a non-success response into an error carrying Meta's own words.
///
/// `error_for_status` consumes the response and reports only the status line,
/// but the Graph API puts the reason in the body — an upload rejected for its
/// size, type or an expired token says just "400 Bad Request" without it,
/// which is not enough to act on.
#[cfg(not(feature = "airtight"))]
async fn checked(
    response: reqwest::Response,
    url: &str,
    doing: &str,
) -> Result<reqwest::Response, FormaError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    tracing::error!(%status, "{doing} failed: {body}");
    Err(FormaError::ApiCall {
        url: Some(url.to_string()),
        msg: format!("{doing}: HTTP {status}: {body}"),
    })
}

/// Fetches an inbound attachment.
///
/// Media arrives as a bare id: the webhook carries no bytes, so this is two
/// round trips — resolve the id to a temporary URL, then fetch that URL with
/// the same bearer token, which Meta requires even though the URL is signed.
#[cfg_attr(feature = "airtight", allow(unused_variables))]
pub(crate) async fn download_media(
    credentials: &WhatsappAPICredentials,
    media_id: &str,
    fallback_mime: &str,
) -> Result<ChatMedia, FormaError> {
    #[cfg(feature = "airtight")]
    {
        // Airtight builds make no network calls.
        Ok(ChatMedia {
            mime_type: fallback_mime.to_string(),
            bytes: Vec::new(),
        })
    }

    #[cfg(not(feature = "airtight"))]
    {
        let client = &*CLIENT;
        let auth = format!("Bearer {}", credentials.access_token);

        let lookup_url = media_lookup_url(media_id);
        let response = client
            .get(&lookup_url)
            .header("Authorization", &auth)
            .send()
            .await?;

        let handle: MediaHandle = checked(
            response,
            &lookup_url,
            &format!("resolving WhatsApp media '{media_id}'"),
        )
        .await?
        .json()
        .await
        .map_err(|e| FormaError::ApiCall {
            url: Some(lookup_url),
            msg: format!("WhatsApp media lookup was not JSON: {e}"),
        })?;

        let response = client
            .get(&handle.url)
            .header("Authorization", &auth)
            .send()
            .await?;

        let bytes = checked(
            response,
            &handle.url,
            &format!("downloading WhatsApp media '{media_id}'"),
        )
        .await?
        .bytes()
        .await?;

        // The lookup is authoritative about the type, but it is not always
        // populated; the webhook's own `mime_type` is the fallback.
        let mime_type = if handle.mime_type.is_empty() {
            fallback_mime.to_string()
        } else {
            handle.mime_type
        };

        Ok(ChatMedia {
            mime_type,
            bytes: bytes.to_vec(),
        })
    }
}

/// Uploads an outbound attachment and returns the id a message can refer to.
///
/// WhatsApp will not take bytes inline, and the `link` form needs a public
/// URL — which a freshly generated image does not have — so anything Forma
/// produces has to be uploaded first.
#[cfg_attr(feature = "airtight", allow(unused_variables))]
pub(crate) async fn upload_media(
    credentials: &WhatsappAPICredentials,
    media: &ChatMedia,
    filename: &str,
) -> Result<String, FormaError> {
    #[cfg(feature = "airtight")]
    {
        // Airtight builds make no network calls.
        Ok(String::new())
    }

    #[cfg(not(feature = "airtight"))]
    {
        let url = media_api_url(&credentials.phone_number_id);
        let part = reqwest::multipart::Part::bytes(media.bytes.clone())
            .file_name(filename.to_string())
            .mime_str(&media.mime_type)
            .map_err(|e| FormaError::Input(format!("invalid media type for upload: {e}")))?;

        let form = reqwest::multipart::Form::new()
            .text("messaging_product", "whatsapp")
            .text("type", media.mime_type.clone())
            .part("file", part);

        tracing::info!(
            bytes = media.bytes.len(),
            mime_type = %media.mime_type,
            "uploading media to WhatsApp"
        );

        let response = CLIENT
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", credentials.access_token),
            )
            .multipart(form)
            .send()
            .await?;

        let uploaded: UploadedMedia = checked(response, &url, "uploading media to WhatsApp")
            .await?
            .json()
            .await
            .map_err(|e| FormaError::ApiCall {
                url: Some(url),
                msg: format!("WhatsApp upload response was not JSON: {e}"),
            })?;

        tracing::info!(media_id = %uploaded.id, "media uploaded");
        Ok(uploaded.id)
    }
}

/// WhatsApp's own limits on an interactive message.
const WA_BODY_LIMIT: usize = 1024;
const WA_HEADER_LIMIT: usize = 60;
fn default_recipient_type() -> String {
    "individual".to_string()
}

fn default_messaging_product() -> String {
    "whatsapp".to_string()
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OutTextBody {
    pub(crate) preview_url: bool,
    pub(crate) body: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OutButtonReply {
    pub(crate) id: String,
    pub(crate) title: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OutButton {
    #[serde(rename = "type")]
    pub(crate) r#type: String,
    pub(crate) reply: OutButtonReply,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OutInteractiveAction {
    pub(crate) buttons: Vec<OutButton>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OutInteractiveBody {
    pub(crate) text: String,
}

/// The bold line WhatsApp shows above the body. Limited to 60 characters.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OutInteractiveHeader {
    #[serde(rename = "type")]
    pub(crate) r#type: String,
    pub(crate) text: String,
}

/// The small grey line under the body. Also limited to 60 characters.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OutInteractiveFooter {
    pub(crate) text: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OutInteractiveMessage {
    #[serde(rename = "type")]
    pub(crate) r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) header: Option<OutInteractiveHeader>,
    pub(crate) body: OutInteractiveBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) footer: Option<OutInteractiveFooter>,
    pub(crate) action: OutInteractiveAction,
}

/// An outbound attachment, referred to by the id its upload returned.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OutMediaBody {
    pub(crate) id: String,
    /// WhatsApp shows this under the picture; omitted when there is no text
    /// to go with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) caption: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum OutBody {
    Text { text: OutTextBody },
    Image { image: OutMediaBody },
    Interactive { interactive: OutInteractiveMessage },
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct WhatsappOutgoingMessage {
    #[serde(default = "default_messaging_product")]
    pub(crate) messaging_product: String,
    #[serde(default = "default_recipient_type")]
    pub(crate) recipient_type: String,
    #[serde(flatten)]
    pub(crate) content: OutBody,

    to: String,
}

#[cfg(not(feature = "airtight"))]
async fn call_whatsapp_cloud_api(c: RequestBuilder) -> Result<(), FormaError> {
    let res = c.send().await?;
    let status = res.status();
    if !status.is_success() {
        let req_e = res.error_for_status_ref().unwrap_err();
        let e = res.text().await?;
        tracing::error!("{}: {}", req_e, e);
        return Err(FormaError::ApiCall {
            url: Some(format!(
                "https://graph.facebook.com/{API_VERSION}/<number>/messages"
            )),
            msg: format!("{req_e}: '{e}'"),
        });
    }
    Ok(())
}

impl WhatsappOutgoingMessage {
    #[tracing::instrument(skip_all)]
    pub(crate) async fn send(
        &self,
        credentials: &WhatsappAPICredentials,
    ) -> Result<(), FormaError> {
        let client = &*CLIENT;
        let url = messages_api_url(&credentials.phone_number_id);
        let mut _c = client.post(url).json(self);
        _c = _c.header(
            "Authorization",
            format!("Bearer {}", credentials.access_token),
        );

        #[cfg(not(feature = "airtight"))]
        call_whatsapp_cloud_api(_c).await?;
        Ok(())
    }

    /// Returns a text message destined to `to` with
    /// a text content of `body`.
    ///
    /// > Note: Enables URL preview.
    pub(crate) fn new_text_to(to: String, body: String) -> Self {
        Self {
            to,
            content: OutBody::Text {
                text: OutTextBody {
                    preview_url: true,
                    body,
                },
            },
            ..Default::default()
        }
    }

    /// Returns an image message for an already-uploaded `media_id`.
    ///
    /// A caption longer than WhatsApp's 1024-character limit is dropped
    /// rather than truncated: the text is sent as its own message instead, so
    /// nothing the agent said goes missing.
    pub(crate) fn new_image_to(to: String, media_id: String, caption: Option<String>) -> Self {
        let caption = caption.filter(|c| !c.trim().is_empty() && c.len() <= 1024);
        Self {
            to,
            content: OutBody::Image {
                image: OutMediaBody {
                    id: media_id,
                    caption,
                },
            },
            ..Default::default()
        }
    }

    /// Returns an interactive button message asking the user to approve or
    /// reject a pending tool call.
    pub(crate) fn new_interactive_approval(to: String, approval: &Approval) -> Self {
        // The action being approved goes in the header, where WhatsApp shows
        // it in bold, leaving the whole body for the details.
        let header = OutInteractiveHeader {
            r#type: "text".to_string(),
            text: truncate_chars(&approval.title, WA_HEADER_LIMIT),
        };

        Self {
            to,
            content: OutBody::Interactive {
                interactive: OutInteractiveMessage {
                    r#type: "button".to_string(),
                    header: Some(header),
                    body: OutInteractiveBody {
                        text: truncate_chars(&approval.details, WA_BODY_LIMIT),
                    },
                    footer: Some(OutInteractiveFooter {
                        text: APPROVAL_PROMPT.to_string(),
                    }),
                    action: OutInteractiveAction {
                        buttons: vec![
                            OutButton {
                                r#type: "reply".to_string(),
                                reply: OutButtonReply {
                                    id: approval.approve.clone(),
                                    title: APPROVE_LABEL.to_string(),
                                },
                            },
                            OutButton {
                                r#type: "reply".to_string(),
                                reply: OutButtonReply {
                                    id: approval.reject.clone(),
                                    title: REJECT_LABEL.to_string(),
                                },
                            },
                        ],
                    },
                },
            },
            ..Default::default()
        }
    }
}

impl Default for WhatsappOutgoingMessage {
    fn default() -> Self {
        Self {
            messaging_product: default_messaging_product(),
            recipient_type: default_recipient_type(),
            to: Default::default(),
            content: OutBody::Text {
                text: OutTextBody {
                    preview_url: Default::default(),
                    body: Default::default(),
                },
            },
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_messages_text() {
        // let m = WhatsappOutgoingMessage {
        //     to: "123123".to_string(),
        //     ..Default::default()
        // };
        // println!("{}", serde_json::to_string(&m).unwrap());

        let payloads = [include_str!("./types/outgoing_text_msg.json")];
        for payload in payloads.iter() {
            serde_json::from_str::<WhatsappOutgoingMessage>(&payload).unwrap();
        }
    }

    fn pending(tool_name: &str, arguments: serde_json::Value) -> protocols::PendingToolCall {
        protocols::PendingToolCall {
            tool_call_id: "call-1".to_string(),
            tool_name: tool_name.to_string(),
            arguments,
            ..Default::default()
        }
    }

    fn approval(arguments: serde_json::Value) -> serde_json::Value {
        let msg = WhatsappOutgoingMessage::new_interactive_approval(
            "5491100000000".to_string(),
            &Approval::new(&pending("make-image", arguments)),
        );
        serde_json::to_value(&msg).expect("serialises")
    }

    #[test]
    fn an_approval_reads_as_a_list_not_as_json() {
        let json = approval(serde_json::json!({
            "prompt": "a lighthouse",
            "count": 2,
            "draft": true,
        }));

        assert_eq!(json["interactive"]["header"]["text"], "🔧 Make image");
        assert_eq!(json["interactive"]["footer"]["text"], "Approve to continue");

        let body = json["interactive"]["body"]["text"].as_str().expect("body");
        // Labels are humanised and values keep neither quotes nor braces.
        assert!(body.contains("*Prompt:* a lighthouse"), "got: {body}");
        assert!(body.contains("*Count:* 2"), "got: {body}");
        assert!(body.contains("*Draft:* yes"), "got: {body}");
        assert!(!body.contains('{'), "no JSON braces: {body}");
        assert!(!body.contains('"'), "no JSON quotes: {body}");
    }

    #[test]
    fn buttons_still_encode_the_routing_ids() {
        // The titles are decoration; the ids are what `handle_interactive`
        // parses, so they must not drift.
        let json = approval(serde_json::json!({ "prompt": "x" }));
        let buttons = json["interactive"]["action"]["buttons"]
            .as_array()
            .expect("buttons");
        assert_eq!(buttons[0]["reply"]["id"], "approve|call-1");
        assert_eq!(buttons[1]["reply"]["id"], "reject|call-1");
        // WhatsApp caps button titles at 20 characters.
        for b in buttons {
            let title = b["reply"]["title"].as_str().expect("title");
            assert!(title.chars().count() <= 20, "too long: {title}");
        }
    }

    #[test]
    fn a_long_value_gets_its_own_line() {
        let long = "Draw a lighthouse at sunset with dramatic clouds and a rocky shore below";
        let json = approval(serde_json::json!({ "prompt": long }));
        let body = json["interactive"]["body"]["text"].as_str().expect("body");
        assert!(body.starts_with("*Prompt*\n"), "got: {body}");
    }

    #[test]
    fn a_nested_value_falls_back_to_monospaced_json() {
        let json = approval(serde_json::json!({ "size": { "w": 10, "h": 20 } }));
        let body = json["interactive"]["body"]["text"].as_str().expect("body");
        assert!(body.contains("```"), "got: {body}");
    }

    #[test]
    fn a_list_of_scalars_reads_as_a_list() {
        let json = approval(serde_json::json!({ "tags": ["sea", "dusk", "calm"] }));
        let body = json["interactive"]["body"]["text"].as_str().expect("body");
        assert!(body.contains("*Tags:* sea, dusk, calm"), "got: {body}");
    }

    #[test]
    fn an_approval_with_no_arguments_still_says_something() {
        let json = approval(serde_json::json!({}));
        assert_eq!(json["interactive"]["body"]["text"], "_No details._");
    }

    #[test]
    fn accented_text_does_not_panic_the_send() {
        // The previous version sliced the body at byte 1021, which panics
        // when that offset lands inside a multi-byte character — as it does
        // for any Spanish prompt long enough to need truncating.
        let long_spanish = "á".repeat(2000);
        let json = approval(serde_json::json!({ "descripción": long_spanish }));
        let body = json["interactive"]["body"]["text"].as_str().expect("body");
        assert!(body.chars().count() <= 1024);
        assert!(body.contains("Descripción"));
    }

    #[test]
    fn every_field_survives_a_long_first_one() {
        // One enormous argument must not push the others out of the message.
        let json = approval(serde_json::json!({
            "aaa_first": "x".repeat(5000),
            "zzz_last": "still here",
        }));
        let body = json["interactive"]["body"]["text"].as_str().expect("body");
        assert!(
            body.contains("still here"),
            "the long field crowded out the rest: {body}"
        );
    }

    #[test]
    fn the_header_stays_within_whatsapps_limit() {
        let msg = WhatsappOutgoingMessage::new_interactive_approval(
            "549110".to_string(),
            &Approval::new(&pending(
                &"a-very-long-tool-name".repeat(10),
                serde_json::json!({}),
            )),
        );
        let json = serde_json::to_value(&msg).expect("serialises");
        let header = json["interactive"]["header"]["text"].as_str().expect("h");
        assert!(
            header.chars().count() <= 60,
            "got {}",
            header.chars().count()
        );
    }

    #[test]
    fn image_message_refers_to_the_uploaded_media() {
        let msg = WhatsappOutgoingMessage::new_image_to(
            "5491100000000".to_string(),
            "media-123".to_string(),
            Some("here you go".to_string()),
        );
        let json: serde_json::Value = serde_json::to_value(&msg).expect("serialises");

        assert_eq!(json["messaging_product"], "whatsapp");
        assert_eq!(json["to"], "5491100000000");
        assert_eq!(json["type"], "image");
        assert_eq!(json["image"]["id"], "media-123");
        assert_eq!(json["image"]["caption"], "here you go");
    }

    #[test]
    fn an_absent_caption_is_left_out_entirely() {
        let json = serde_json::to_value(WhatsappOutgoingMessage::new_image_to(
            "549110".to_string(),
            "m1".to_string(),
            None,
        ))
        .expect("serialises");
        assert!(json["image"].get("caption").is_none());

        // Blank text is the same as none.
        let json = serde_json::to_value(WhatsappOutgoingMessage::new_image_to(
            "549110".to_string(),
            "m1".to_string(),
            Some("   ".to_string()),
        ))
        .expect("serialises");
        assert!(json["image"].get("caption").is_none());
    }

    #[test]
    fn an_overlong_caption_is_dropped_rather_than_truncated() {
        // WhatsApp caps captions at 1024 characters. Cutting the agent's
        // answer in half would lose meaning, so it travels as its own
        // message instead — `send_reply` notices the caption was refused.
        let json = serde_json::to_value(WhatsappOutgoingMessage::new_image_to(
            "549110".to_string(),
            "m1".to_string(),
            Some("x".repeat(1025)),
        ))
        .expect("serialises");
        assert!(json["image"].get("caption").is_none());
    }
}

#[cfg(all(test, feature = "integration_test"))]
mod integration_test {
    #[tokio::test]
    async fn test_send_msg() {
        todo!()
    }
}
