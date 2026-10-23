// https://developers.secure.facebook.com/documentation/business-messaging/whatsapp/reference/webhooks/whatsapp-incoming-webhook-payload

use serde::{Deserialize, Serialize};

use super::incomming_message::WhatsappIncomingMessage;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappReferredProduct {
    /// Product catalog ID.
    catalog_id: String,
    /// Product ID.
    product_retailer_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub(crate) struct WhatsappMessageContext {
    /// Business display phone number this person is
    /// responding to (probably yours)
    from: Option<String>,
    /// WhatsApp message ID of the message the
    /// user used to access the "Message business" button.
    /// (e.g., the message this is responding to)
    id: Option<String>,
    #[serde(default)]
    referred_product: Option<WhatsappReferredProduct>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TextBody {
    pub(crate) body: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ImageBody {
    /// Only present when the sender typed one alongside the picture.
    #[serde(default)]
    pub(crate) caption: String,
    pub(crate) mime_type: String,
    #[serde(default)]
    pub(crate) sha256: String,
    /// The handle the media has to be downloaded with; an inbound webhook
    /// carries no bytes and no URL of its own.
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) static_url: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct LocationBody {
    latitude: f64,
    longitude: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ReactionBody {
    /// WhatsApp message ID of the message the WhatsApp user reacted to.
    pub(crate) message_id: String,
    /// Emoji sent by the WhatsApp user as a reaction.
    pub(crate) emoji: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct AudioBody {
    pub(crate) id: String,
    pub(crate) mime_type: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ContactName {
    pub(crate) formatted_name: String,
    #[serde(default)]
    pub(crate) first_name: String,
    #[serde(default)]
    pub(crate) last_name: String,
    #[serde(default)]
    pub(crate) middle_name: String,
    #[serde(default)]
    pub(crate) suffix: String,
    #[serde(default)]
    pub(crate) prefix: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ContactPhone {
    #[serde(default)]
    pub(crate) phone: String,
    #[serde(default)]
    pub(crate) wa_id: String,
    #[serde(rename = "type")]
    pub(crate) r#type: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct Contact {
    pub(crate) name: ContactName,
    pub(crate) phones: Vec<ContactPhone>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct DocumentBody {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) caption: String,
    #[serde(default)]
    pub(crate) filename: String,
    pub(crate) sha256: String,
    pub(crate) mime_type: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ButtonReply {
    pub(crate) id: String,
    pub(crate) title: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ListReply {
    pub(crate) id: String,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) description: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InteractiveContent {
    ButtonReply { button_reply: ButtonReply },
    ListReply { list_reply: ListReply },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct OrderProductItem {
    pub(crate) product_retailer_id: String,
    pub(crate) quantity: String,
    pub(crate) item_price: String,
    pub(crate) currency: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct OrderBody {
    pub(crate) catalog_id: String,
    pub(crate) text: String,
    pub(crate) product_items: Vec<OrderProductItem>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct StickerBody {
    pub(crate) id: String,
    pub(crate) mime_type: String,
    pub(crate) sha256: String,
    pub(crate) animated: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct VideoBody {
    #[serde(default)]
    pub(crate) caption: String,
    pub(crate) mime_type: String,
    pub(crate) sha256: String,
    pub(crate) id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ButtonBody {
    pub(crate) payload: String,
    pub(crate) text: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
#[derive(Default)]
pub(crate) enum WhatsappContent {
    Text {
        text: TextBody,
    },
    Image {
        image: ImageBody,
    },
    Location {
        location: LocationBody,
    },
    Reaction {
        reaction: ReactionBody,
    },
    Audio {
        audio: AudioBody,
    },
    Contacts {
        contacts: Vec<Contact>,
    },
    Document {
        document: DocumentBody,
    },
    Interactive {
        interactive: InteractiveContent,
    },
    Order {
        order: OrderBody,
    },
    Sticker {
        sticker: StickerBody,
    },
    System {
        system: WhatsappSystem,
    },
    Video {
        video: VideoBody,
    },
    Button {
        button: ButtonBody,
    },
    Unsupported {
        errors: Vec<WhatsappStatusError>,
    },
    /// Any `type` this build does not model.
    ///
    /// Meta adds message types without warning, and the webhook is a single
    /// payload covering several messages: without this catch-all one unknown
    /// type rejects the whole request, and a webhook that keeps failing is one
    /// Meta eventually disables.
    #[default]
    #[serde(other)]
    Unknown,
}

fn default_system_type() -> String {
    "user_changed_number".to_string()
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappSystem {
    /// Description of the system change (e.g., user changed number).
    body: String,

    /// New WhatsApp user ID.
    wa_id: String,

    /// Type of system message. 'user_changed_number'
    #[serde(rename = "type", default = "default_system_type")]
    system_type: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WhatsappProfile {
    /// WhatsApp user's name as it appears in their
    /// profile in the WhatsApp client.
    name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappContactProfile {
    /// WhatsApp user ID. Note that a WhatsApp user's
    /// ID and phone number may not always match.
    wa_id: String,

    /// The user profile
    profile: WhatsappProfile,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappMetadata {
    /// Business display phone number.
    display_phone_number: String,

    /// Business phone number ID.
    pub(crate) phone_number_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct IncomingMessageValueGeneral {
    /// Always 'whatsapp'
    #[serde(default = "default_whatsapp_messaging_product")]
    messaging_product: String,

    /// Metadata
    pub(crate) metadata: WhatsappMetadata,

    /// Array of contact profiles for the sender. Included
    /// for all non-system incoming messages.
    #[serde(default)]
    pub(crate) contacts: Vec<WhatsappContactProfile>,

    /// Array of message objects. The structure varies based on the 'type' property.
    #[serde(default)]
    pub(crate) messages: Vec<WhatsappIncomingMessage>,

    #[serde(default)]
    pub(crate) statuses: Vec<WhatsappStatusChange>,
}

#[derive(Debug, Clone)]
pub enum MessageWrapper {
    /// Standard messages (text, image, etc.) and statuses
    General(IncomingMessageValueGeneral),
}

// Delegated rather than `untagged`: with a single variant `untagged` buys
// nothing, and it swallows the real parse failure — every problem became
// "data did not match any variant of untagged enum MessageWrapper", which
// names neither the field nor the reason. Delegating propagates serde's own
// message, which is what makes a malformed webhook diagnosable.
impl Serialize for MessageWrapper {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::General(v) => v.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for MessageWrapper {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        IncomingMessageValueGeneral::deserialize(deserializer).map(Self::General)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WhatsappMsgStatus {
    #[default]
    Sent,
    Delivered,
    Read,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappErrorData {
    #[serde(default)]
    pub(crate) details: String,
}

/// Why Meta rejected or failed to deliver a message.
///
/// Only `code` is dependable: which of the rest Meta includes varies by error
/// and by API version, and requiring them meant a failed-delivery webhook
/// could not be parsed at all — throwing away the one payload that says *why*
/// a send did not arrive.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappStatusError {
    pub(crate) code: usize,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) error_data: Option<WhatsappErrorData>,
    #[serde(default)]
    pub(crate) href: String,
}

impl std::fmt::Display for WhatsappStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}]", self.code)?;
        if !self.title.is_empty() {
            write!(f, " {}", self.title)?;
        }
        if !self.message.is_empty() && self.message != self.title {
            write!(f, ": {}", self.message)?;
        }
        if let Some(data) = &self.error_data {
            write!(f, " ({})", data.details)?;
        }
        if !self.href.is_empty() {
            write!(f, " — see {}", self.href)?;
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "UPPERCASE")]
enum WhatsappPricingModel {
    Cbp,
    Pmp,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappPricing {
    billable: bool,
    pricing_model: WhatsappPricingModel,
    category: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WhatsapConversationOrigin {
    #[serde(rename = "type")]
    origin_type: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappConversation {
    id: String,
    expiration_timestamp: Option<String>,
    origin: WhatsapConversationOrigin,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappStatusChange {
    /// Unique WhatsApp message ID the status is associated with.
    pub(crate) id: String,
    /// One of "sent", "delivered", "read", "failed"
    pub(crate) status: WhatsappMsgStatus,
    pub(crate) timestamp: String,
    /// Recipeint phone number.
    pub(crate) recipient_id: String,
    /// Group ID if the message was sent to a group.
    pub(crate) group_id: Option<String>,

    pub(crate) conversation: Option<WhatsappConversation>,
    pub(crate) pricing: Option<WhatsappPricing>,
    #[serde(default)]
    pub(crate) errors: Vec<WhatsappStatusError>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
enum WhatsappGroupChangeType {
    #[serde(rename = "group_create")]
    Create,
    #[serde(rename = "group_delete")]
    Delete,
    #[serde(rename = "group_settings_update")]
    SettingsUpdate,
    #[serde(rename = "group_add_participants")]
    AddParticipants,
    #[serde(rename = "group_remove_participants")]
    RemoveParticipants,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WhatsappGroupParticipant {
    /// Input phone number or WhatsApp ID
    input: String,

    /// WhatsApp ID of the participant
    wa_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WhatsappGroupProfilePicture {
    /// MIME type of the profile picture
    mime_type: String,

    /// SHA256 hash of the profile picture
    sha256: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappGroup {
    /// Unix timestamp of the group event
    timestamp: String,
    /// Unique identifier for the group
    group_id: String,
    /// Type of group event. One of "group_create", "group_delete", "group_settings_update", "group_add_participants", "group_remove_participants"
    group_change_type: WhatsappGroupChangeType,
    /// Unique identifier for the request
    request_id: String,
    /// Group subject/name
    subject: String,
    /// Group description
    description: String,
    /// List of participants added to the group
    added_participants: Vec<WhatsappGroupParticipant>,

    /// List of participants added to the group
    removed_participants: Vec<WhatsappGroupParticipant>,

    /// List of participants added to the group
    profile_picture: WhatsappGroupProfilePicture,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct GroupValue {
    /// Always 'whatsapp'
    #[serde(default = "default_whatsapp_messaging_product")]
    messaging_product: String,

    /// Metadata
    pub(crate) metadata: WhatsappMetadata,

    /// Groups
    pub(crate) groups: Vec<WhatsappGroup>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct MessageTemplateStatusUpdate {
    pub(crate) message_template_id: String,
    pub(crate) message_template_name: String,
    pub(crate) message_template_language: String,
    pub(crate) event: String,
    pub(crate) reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct AccountReviewUpdate {
    pub(crate) review_status: String,
    pub(crate) decision_reason: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct AccountUpdate {
    pub(crate) event: String,
    pub(crate) phone_number: String,
    pub(crate) country: String,
    pub(crate) previous_country: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TemplateCategoryUpdate {
    pub(crate) message_template_id: String,
    pub(crate) message_template_name: String,
    pub(crate) message_template_language: String,
    pub(crate) event: String,
    pub(crate) new_category: String,
    pub(crate) old_category: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct MessageTemplateQualityUpdate {
    pub(crate) message_template_id: String,
    pub(crate) message_template_name: String,
    pub(crate) message_template_language: String,
    pub(crate) event: String,
    pub(crate) new_quality_score: String,
    pub(crate) old_quality_score: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct PhoneNumberNameUpdate {
    pub(crate) display_phone_number: String,
    pub(crate) event: String,
    pub(crate) new_name: String,
    pub(crate) old_name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct PhoneNumberQualityUpdate {
    pub(crate) display_phone_number: String,
    pub(crate) event: String,
    pub(crate) new_quality: String,
    pub(crate) old_quality: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct BusinessCapabilityUpdate {
    pub(crate) business_capability_type: String,
    pub(crate) event: String,
    pub(crate) new_capability: String,
    pub(crate) old_capability: String,
}

/// A change, deserialized leniently by [`WhatsappChange`]'s own impl below.
#[derive(Deserialize)]
struct RawChange {
    field: String,
    #[serde(default)]
    value: serde_json::Value,
}

impl<'de> Deserialize<'de> for WhatsappChange {
    /// Reads the `field` first and only then the `value`, so that neither an
    /// unrecognised field nor a known field carrying an unexpected shape can
    /// reject the request.
    ///
    /// The derived adjacently-tagged form could not do this: `#[serde(other)]`
    /// only produces a unit variant, and serde still tried to read `value`
    /// into it. A webhook that answers 422 is one Meta retries and eventually
    /// turns off, so being lenient here is what keeps the integration alive.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawChange::deserialize(deserializer)?;

        fn parse<T: for<'a> Deserialize<'a>>(value: serde_json::Value, field: &str) -> Option<T> {
            match serde_json::from_value(value) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!("could not read the '{field}' change: {e}");
                    None
                }
            }
        }

        let field = raw.field.as_str();
        let change = match field {
            "messages" => parse(raw.value, field).map(Self::Messages),
            "group_lifecycle_update" => parse(raw.value, field).map(Self::GroupLifecycle),
            "group_settings_update" => parse(raw.value, field).map(Self::GroupSettings),
            "group_participant_update" => parse(raw.value, field).map(Self::GroupParticipant),
            "message_template_status_update" => {
                parse(raw.value, field).map(Self::MessageTemplateStatusUpdate)
            }
            "account_review_update" => parse(raw.value, field).map(Self::AccountReviewUpdate),
            "account_update" => parse(raw.value, field).map(Self::AccountUpdate),
            "template_category_update" => parse(raw.value, field).map(Self::TemplateCategoryUpdate),
            "message_template_quality_update" => {
                parse(raw.value, field).map(Self::MessageTemplateQualityUpdate)
            }
            "phone_number_name_update" => parse(raw.value, field).map(Self::PhoneNumberNameUpdate),
            "phone_number_quality_update" => {
                parse(raw.value, field).map(Self::PhoneNumberQualityUpdate)
            }
            "business_capability_update" => {
                parse(raw.value, field).map(Self::BusinessCapabilityUpdate)
            }
            other => {
                tracing::warn!("received an unrecognised WhatsApp change field '{other}'");
                None
            }
        };

        Ok(change.unwrap_or(Self::Unknown))
    }
}

#[derive(Serialize, Debug, Clone)]
#[serde(tag = "field", content = "value", rename_all = "snake_case")]
pub enum WhatsappChange {
    /// The "messages" field covers both user messages and system events
    #[serde(rename = "messages")]
    Messages(MessageWrapper),

    /// Group-related updates often share the same data structure (GroupValue)
    #[serde(rename = "group_lifecycle_update")]
    GroupLifecycle(GroupValue),

    #[serde(rename = "group_settings_update")]
    GroupSettings(GroupValue),

    #[serde(rename = "group_participant_update")]
    GroupParticipant(GroupValue),

    #[serde(rename = "message_template_status_update")]
    MessageTemplateStatusUpdate(MessageTemplateStatusUpdate),

    #[serde(rename = "account_review_update")]
    AccountReviewUpdate(AccountReviewUpdate),

    #[serde(rename = "account_update")]
    AccountUpdate(AccountUpdate),

    #[serde(rename = "template_category_update")]
    TemplateCategoryUpdate(TemplateCategoryUpdate),

    #[serde(rename = "message_template_quality_update")]
    MessageTemplateQualityUpdate(MessageTemplateQualityUpdate),

    #[serde(rename = "phone_number_name_update")]
    PhoneNumberNameUpdate(PhoneNumberNameUpdate),

    #[serde(rename = "phone_number_quality_update")]
    PhoneNumberQualityUpdate(PhoneNumberQualityUpdate),

    #[serde(rename = "business_capability_update")]
    BusinessCapabilityUpdate(BusinessCapabilityUpdate),

    /// Any `field` this build does not model, or a known one whose payload
    /// could not be read. Kept for the same reason as
    /// [`WhatsappContent::Unknown`].
    Unknown,
}

/// 'whatsapp'
fn default_whatsapp_messaging_product() -> String {
    "whatsapp".to_string()
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappPayloadEntry {
    /// WhatsApp Business Account ID.
    id: String,
    /// The changes
    pub(crate) changes: Vec<WhatsappChange>,
}

/// The payload sent by Whatsapp to the `/whatsapp/webhook` endpoint
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct WhatsappWebhookPayload {
    /// always 'whatsapp_business_account'
    object: String,

    /// A list of entries.
    pub(crate) entry: Vec<WhatsappPayloadEntry>,
}

#[cfg(test)]
pub(crate) fn create_test_payload(message: &str) -> String {
    format!(
        r#"{{
            "object": "whatsapp_business_account",
            "entry": [
                {{
                "id": "872024278909394",
                "changes": [
                    {{
                    "value": {{
                        "messaging_product": "whatsapp",
                        "metadata": {{
                            "display_phone_number": "15551658695",
                            "phone_number_id": "912164528653259"
                        }},
                        "contacts": [
                            {{
                                "profile": {{
                                "name": "German"
                                }},
                                "wa_id": "64272815246"
                            }}
                        ],
                        "messages": [
                            {}
                        ]
                    }},
                    "field": "messages"
                    }}
                ]
                }}
            ]
            }}"#,
        message
    )
}

#[cfg(test)]
mod testing {

    use super::*;

    fn create_test_change_payload(change: &str) -> String {
        format!(
            r#"{{
                "object": "whatsapp_business_account",
                "entry": [
                    {{
                        "id": "872024278909394",
                        "changes": [
                            {}
                        ]
                    }}
                ]
            }}"#,
            change
        )
    }

    #[test]
    fn test_message_reaction() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "reaction",
            "reaction": {
                "message_id": "wamid.HBgPMjY1MzY4OTMzMjY5NTgxFQIAERgSN0VBRUZCMjVBMDRGN0QzQUY5AA==",
                "emoji": "❤️"
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_status_delivered() {
        let change: WhatsappStatusChange = serde_json::from_str(
            r#"{
                                "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgARGBI3RUFFRkIyNUEwNEY3RDNBRjkA",
                                "status": "delivered",
                                "timestamp": "1767994574",
                                "recipient_id": "64272815246",
                                "conversation": {
                                    "id": "1d90d8f505e84b6c9fcf7a8551f25428",
                                    "origin": {
                                        "type": "marketing"
                                    }
                                },
                                "pricing": {
                                "billable": true,
                                "pricing_model": "PMP",
                                "category": "marketing"
                                }
                            }"#,
        )
        .unwrap();

        let _s = MessageWrapper::General(IncomingMessageValueGeneral {
            messaging_product: default_whatsapp_messaging_product(),
            metadata: WhatsappMetadata {
                display_phone_number: "some number".to_string(),
                phone_number_id: "the number id".to_string(),
            },
            contacts: vec![],
            messages: vec![],
            statuses: vec![change],
        });

        let payload = r#"{
            "object": "whatsapp_business_account",
            "entry": [
                {
                "id": "872024278909394",
                "changes": [
                    {
                    "value": {
                        "messaging_product": "whatsapp",
                        "metadata": {
                            "display_phone_number": "15551658695",
                            "phone_number_id": "912164528653259"
                        },
                        "statuses": [
                            {
                                "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgARGBI3RUFFRkIyNUEwNEY3RDNBRjkA",
                                "status": "delivered",
                                "timestamp": "1767994574",
                                "recipient_id": "64272815246",
                                "conversation": {
                                "id": "1d90d8f505e84b6c9fcf7a8551f25428",
                                "origin": {
                                    "type": "marketing"
                                }
                                },
                                "pricing": {
                                "billable": true,
                                "pricing_model": "PMP",
                                "category": "marketing"
                                }
                            }
                        ]
                    },
                    "field": "messages"
                    }
                ]
                }
            ]
            }"#;
        serde_json::from_str::<WhatsappWebhookPayload>(payload).unwrap();
    }

    #[test]
    fn test_messages_status_sent() {
        let payload = r#"{
            "object": "whatsapp_business_account",
            "entry": [
                {
                "id": "872024278909394",
                "changes": [
                    {
                    "value": {
                        "messaging_product": "whatsapp",
                        "metadata": {
                        "display_phone_number": "15551658695",
                        "phone_number_id": "912164528653259"
                        },
                        "statuses": [
                        {
                            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgARGBI3RUFFRkIyNUEwNEY3RDNBRjkA",
                            "status": "sent",
                            "timestamp": "1767994573",
                            "recipient_id": "64272815246",
                            "conversation": {
                            "id": "1d90d8f505e84b6c9fcf7a8551f25428",
                            "expiration_timestamp": "1767994573",
                            "origin": {
                                "type": "marketing"
                            }
                            },
                            "pricing": {
                            "billable": true,
                            "pricing_model": "PMP",
                            "category": "marketing"
                            }
                        }
                        ]
                    },
                    "field": "messages"
                    }
                ]
                }
            ]
            }"#;
        serde_json::from_str::<WhatsappWebhookPayload>(payload).unwrap();
    }

    #[test]
    fn test_messages_location() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQUREMDg2RkVEQUU2QTMxN0JDMgA=",
            "timestamp": "1767993382",
            "location": {
            "latitude": -41.2912940979,
            "longitude": 174.74331665039
            },
            "type": "location"
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_messages_image() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQUJDNUMxQkQzRjEzRUQwRjYyMQA=",
            "timestamp": "1767993370",
            "type": "image",
            "image": {
            "caption": "Esto",
            "mime_type": "image/jpeg",
            "sha256": "sBvqQ2GVm25xIVxTkkWY96pOmfeKvcSavApBBB3Wehw=",
            "id": "730657879727889",
            "url": "https://lookaside.fbsbx.com/whatsapp_business/attachments/?mid=730657879727889&source=webhook&ext=1767993672&hash=ARk2NWXvtqlx-Rj8NiBtPjPV95pRjO1ZKmyaQ2MiGaqZVg",
            "static_url": "https://interncache-rva.fbcdn.net/m1/v/t6/An9O9YtTqK5x6h6-GS5bYTZBg6f04X64bR3PHdncHT29wSBqNfQIJ4oR5LKaJYtE-NmsT3uKrAMkmP37pG2VkoEPYUjE-nBMMqaAKhk5o3pFhoCUITqRJU12rfGFdwxS8RIc0tVZsA?mdk2=Ae8692-Cu9yDAbASRTMMalvAUA83rpg8GrluruEcR8HDG4cHF3Our8GbSO4RIoaN5SizJbn91Bo9ZhQeQPJjhvFngLLtTbrQuvVAta-limmwL6KuJ4Uu7DDJoEUgkpP_mw6ol-7mZsX5M2hHtSrFQ_wjY2UEYcZKlpk3t33Acu-G4KDIB6zooJ9Q3JeWgu-nTkg&mdktag=1&efg=eyJ1cmxnZW4iOiJtYW5pZm9sZF9jcHBfdXJsZ2VuL0VudFdoYXRzQXBwQnVzaW5lc3NNZXNzYWdlTWVkaWFBdHRhY2htZW50TWFudWFsVHJhaXQifQ&_nc_gid=Uk-RFbAVGo7E6C09CVLQGQ&ccb=10-5&oh=00_AfpRku0y96QLYXRkBPNSzqAxuO_flKjY-paTbVQxUBMCKg&oe=69633302&_nc_sid=850210"
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_messages_text() {
        let payloads = [
            // include_str!("./types/incoming_reaction_emoji.json"),
            include_str!("./types/incoming_reaction_no_emoji.json"),
            include_str!("./types/incoming_status_delivered.json"),
            include_str!("./types/incoming_status_failed.json"),
            include_str!("./types/incoming_status_sent.json"),
            include_str!("./types/incoming_text_message_business.json"),
            include_str!("./types/incoming_text_message_forwarded.json"),
            include_str!("./types/incoming_text_message_no_context.json"),
            include_str!("./types/incoming_text_message_just_text.json"),
        ];
        for payload in payloads.iter() {
            serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
        }
    }

    #[test]
    fn test_message_audio() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "audio",
            "audio": {
                "id": "some_id",
                "mime_type": "audio/ogg"
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    /// Builds a `messages` change around an arbitrary `value` body.
    fn messages_change(value: &str) -> String {
        format!(
            r#"{{"object":"whatsapp_business_account","entry":[{{"id":"1","changes":[{{"field":"messages","value":{value}}}]}}]}}"#
        )
    }

    const META: &str = r#""messaging_product":"whatsapp","metadata":{"display_phone_number":"1555","phone_number_id":"912"}"#;

    #[test]
    fn a_failed_delivery_reports_why_instead_of_being_dropped() {
        // This is the webhook Meta sends when a send does not arrive — the one
        // payload that says *why*. Requiring `href` (which Meta often omits)
        // made it unparseable, so the reason was thrown away and an image that
        // WhatsApp had rejected looked like nothing had happened at all.
        let value = format!(
            r#"{{{META},"statuses":[{{"id":"wamid.X","status":"failed","timestamp":"1767994843","recipient_id":"642728","errors":[{{"code":131053,"title":"Media upload error","message":"Unsupported file type","error_data":{{"details":"the file type is not supported"}}}}]}}]}}"#
        );

        let parsed: WhatsappWebhookPayload =
            serde_json::from_str(&messages_change(&value)).expect("a failed status must parse");

        let WhatsappChange::Messages(MessageWrapper::General(v)) = &parsed.entry[0].changes[0]
        else {
            panic!("a failed status must not fall back to Unknown");
        };

        let error = &v.statuses[0].errors[0];
        assert_eq!(error.code, 131053);
        // And it renders as something worth putting in a log line.
        let rendered = error.to_string();
        assert!(rendered.contains("131053"), "got: {rendered}");
        assert!(rendered.contains("Media upload error"), "got: {rendered}");
        assert!(rendered.contains("not supported"), "got: {rendered}");
    }

    #[test]
    fn an_error_with_only_a_code_still_parses() {
        // Which fields Meta includes varies by error and API version, so only
        // the code can be relied on.
        let value = format!(
            r#"{{{META},"statuses":[{{"id":"wamid.X","status":"failed","timestamp":"1","recipient_id":"6","errors":[{{"code":131000}}]}}]}}"#
        );
        let parsed: WhatsappWebhookPayload =
            serde_json::from_str(&messages_change(&value)).expect("parses");
        let WhatsappChange::Messages(MessageWrapper::General(v)) = &parsed.entry[0].changes[0]
        else {
            panic!("expected a messages change");
        };
        assert_eq!(v.statuses[0].errors[0].to_string(), "[131000]");
    }

    #[test]
    fn a_status_for_a_sent_message_parses_with_its_billing_fields() {
        let value = format!(
            r#"{{{META},"statuses":[{{"id":"wamid.X","status":"sent","timestamp":"1767994843","recipient_id":"642728","conversation":{{"id":"c1","origin":{{"type":"service"}}}},"pricing":{{"billable":true,"pricing_model":"CBP","category":"service"}}}}]}}"#
        );
        serde_json::from_str::<WhatsappWebhookPayload>(&messages_change(&value)).expect("parses");
    }

    #[test]
    fn the_echo_of_an_outgoing_image_parses() {
        // WhatsApp echoes what the business sent; failing on it would make
        // every generated picture log a warning after it went out fine.
        let value = format!(
            r#"{{{META},"messages":[{{"from":"912","id":"wamid.Y","timestamp":"1767994843","type":"image","image":{{"mime_type":"image/png","sha256":"abc","id":"media-1"}}}}]}}"#
        );
        serde_json::from_str::<WhatsappWebhookPayload>(&messages_change(&value)).expect("parses");
    }

    #[test]
    fn an_unknown_message_type_does_not_reject_the_payload() {
        // Meta adds message types without warning. One unknown type used to
        // fail the whole webhook with a 422, and a webhook that keeps failing
        // is one Meta eventually disables — so the rest of the payload has to
        // survive it.
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "some_type_meta_invented_last_week",
            "some_type_meta_invented_last_week": { "whatever": true }
        }"#;
        let payload = create_test_payload(message);
        let parsed: WhatsappWebhookPayload = serde_json::from_str(&payload).unwrap();

        // And it is recognisably "not something we handle", rather than
        // being mistaken for a message with content.
        let WhatsappChange::Messages(MessageWrapper::General(v)) = &parsed.entry[0].changes[0]
        else {
            panic!("expected a messages change");
        };
        assert!(matches!(v.messages[0].content, WhatsappContent::Unknown));
    }

    #[test]
    fn an_unknown_change_field_does_not_reject_the_payload() {
        let payload = r#"{
            "object": "whatsapp_business_account",
            "entry": [{
                "id": "872024278909394",
                "changes": [{
                    "field": "a_field_meta_invented_last_week",
                    "value": { "anything": [1, 2, 3] }
                }]
            }]
        }"#;
        let parsed: WhatsappWebhookPayload = serde_json::from_str(payload).unwrap();
        assert!(matches!(
            parsed.entry[0].changes[0],
            WhatsappChange::Unknown
        ));
    }

    #[test]
    fn test_message_image() {
        // The real inbound shape: a handle and a type, no URL and no bytes.
        // Requiring `url`/`static_url` here used to make every incoming
        // picture fail to parse before it reached the agent.
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "image",
            "image": {
                "mime_type": "image/jpeg",
                "sha256": "abc123",
                "id": "1234567890"
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_image_with_caption() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "image",
            "image": {
                "caption": "what is this?",
                "mime_type": "image/jpeg",
                "sha256": "abc123",
                "id": "1234567890"
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_contacts() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "contacts",
            "contacts": [
                {
                    "name": {
                        "formatted_name": "German",
                        "first_name": "German"
                    },
                    "phones": [
                        {
                            "phone": "+64272815246",
                            "type": "HOME"
                        }
                    ]
                }
            ]
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_document() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "document",
            "document": {
                "id": "some_id",
                "mime_type": "application/pdf",
                "sha256": "some_sha",
                "filename": "some_file.pdf",
                "caption": "some caption"
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_interactive_button() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "interactive",
            "interactive": {
                "type": "button_reply",
                "button_reply": {
                    "id": "some_id",
                    "title": "some_title"
                }
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_interactive_list() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "interactive",
            "interactive": {
                "type": "list_reply",
                "list_reply": {
                    "id": "some_id",
                    "title": "some_title",
                    "description": "some_description"
                }
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_order() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "order",
            "order": {
                "catalog_id": "some_id",
                "text": "some_text",
                "product_items": [
                    {
                        "product_retailer_id": "some_id",
                        "quantity": "1",
                        "item_price": "10",
                        "currency": "USD"
                    }
                ]
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_sticker() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "sticker",
            "sticker": {
                "id": "some_id",
                "mime_type": "image/webp",
                "sha256": "some_sha",
                "animated": false
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_system() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "system",
            "system": {
                "body": "The user has changed their phone number.",
                "wa_id": "64272815246",
                "type": "user_changed_number"
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_video() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "video",
            "video": {
                "id": "some_id",
                "mime_type": "video/mp4",
                "sha256": "some_sha",
                "caption": "some caption"
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_button() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "button",
            "button": {
                "payload": "some_payload",
                "text": "some_text"
            }
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_message_unsupported() {
        let message = r#"{
            "from": "64272815246",
            "id": "wamid.HBgLNjQyNzI4MTUyNDYVAgASGBQzQTVFRkVENTgxREY0NDg3MDI4RAA=",
            "timestamp": "1767994843",
            "type": "unsupported",
            "errors": [
                {
                    "code": 131051,
                    "title": "Message type is not supported",
                    "message": "Message type is not supported",
                    "error_data": {
                        "details": "Message type is not supported"
                    },
                    "href": "https://developers.facebook.com/docs/whatsapp/cloud-api/support/error-codes/"
                }
            ]
        }"#;
        let payload = create_test_payload(message);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_change_message_template_status_update() {
        let change = r#"{
            "field": "message_template_status_update",
            "value": {
                "message_template_id": "123456",
                "message_template_name": "my_template",
                "message_template_language": "en_US",
                "event": "APPROVED"
            }
        }"#;
        let payload = create_test_change_payload(change);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_change_account_review_update() {
        let change = r#"{
            "field": "account_review_update",
            "value": {
                "review_status": "APPROVED",
                "decision_reason": "some_reason"
            }
        }"#;
        let payload = create_test_change_payload(change);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_change_account_update() {
        let change = r#"{
            "field": "account_update",
            "value": {
                "event": "VERIFIED_ACCOUNT",
                "phone_number": "15551658695",
                "country": "US",
                "previous_country": "US"
            }
        }"#;
        let payload = create_test_change_payload(change);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_change_template_category_update() {
        let change = r#"{
            "field": "template_category_update",
            "value": {
                "message_template_id": "123456",
                "message_template_name": "my_template",
                "message_template_language": "en_US",
                "event": "CATEGORY_UPDATE",
                "new_category": "MARKETING",
                "old_category": "UTILITY"
            }
        }"#;
        let payload = create_test_change_payload(change);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_change_message_template_quality_update() {
        let change = r#"{
            "field": "message_template_quality_update",
            "value": {
                "message_template_id": "123456",
                "message_template_name": "my_template",
                "message_template_language": "en_US",
                "event": "QUALITY_SCORE_UPDATE",
                "new_quality_score": "GREEN",
                "old_quality_score": "YELLOW"
            }
        }"#;
        let payload = create_test_change_payload(change);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_change_phone_number_name_update() {
        let change = r#"{
            "field": "phone_number_name_update",
            "value": {
                "display_phone_number": "15551658695",
                "event": "NAME_UPDATE",
                "new_name": "new_name",
                "old_name": "old_name"
            }
        }"#;
        let payload = create_test_change_payload(change);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_change_phone_number_quality_update() {
        let change = r#"{
            "field": "phone_number_quality_update",
            "value": {
                "display_phone_number": "15551658695",
                "event": "QUALITY_UPDATE",
                "new_quality": "GREEN",
                "old_quality": "YELLOW"
            }
        }"#;
        let payload = create_test_change_payload(change);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }

    #[test]
    fn test_change_business_capability_update() {
        let change = r#"{
            "field": "business_capability_update",
            "value": {
                "business_capability_type": "PHONE_NUMBER",
                "event": "CAPABILITY_UPDATE",
                "new_capability": "ENABLED",
                "old_capability": "DISABLED"
            }
        }"#;
        let payload = create_test_change_payload(change);
        serde_json::from_str::<WhatsappWebhookPayload>(&payload).unwrap();
    }
}
