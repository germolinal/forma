use axum::{
    extract::{FromRequest, Json, Request},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use genai_client::{GenaiMessageTrait, Role};
use serde_json::json;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum MessageRejection {
    BadJson(String),
    WrongRole,
    NoContent,
}

impl IntoResponse for MessageRejection {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            MessageRejection::BadJson(s) => (StatusCode::BAD_REQUEST, s.to_string()),
            MessageRejection::WrongRole => (
                StatusCode::BAD_REQUEST,
                "invalid role. input messages should have role='user' or role=null".to_string(),
            ),
            MessageRejection::NoContent => (
                StatusCode::BAD_REQUEST,
                "the received message has no content".to_string(),
            ),
        };

        // Return a JSON response with the error details and the correct status code
        let body = Json(json!({
            "error": error_message,
        }));

        (status, body).into_response()
    }
}

/// This is basically a copy of Axum's Json extractor,
/// but validating that the message has content
#[allow(dead_code)]
pub(crate) struct UserChatMessage<T: GenaiMessageTrait>(pub T);

impl<T, S> FromRequest<S> for UserChatMessage<T>
where
    T: GenaiMessageTrait,
    S: Send + Sync,
{
    type Rejection = MessageRejection;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // Validate as JSON first
        let s: Json<T> = Json::from_request(req, state)
            .await
            .map_err(|x| MessageRejection::BadJson(x.to_string()))?;

        #[cfg(debug_assertions)]
        println!("{}", serde_json::to_string(&s.0).unwrap());

        if !s.has_content() {
            return Err(MessageRejection::NoContent);
        }
        if s.role() != Role::User {
            return Err(MessageRejection::WrongRole);
        }

        Ok(Self(s.0))
    }
}
