use serde::{Deserialize, Serialize};

/// Specifies the output audio format. Must be one of wav, mp3, flac, opus, or pcm16.
#[derive(Default, Debug, Serialize, Deserialize, Copy, Clone)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum AudioFormat {
    Wav,
    Mp3,
    Flac,
    Opus,
    #[default]
    Pcm16,
}

impl AudioFormat {
    /// The IANA media type for this format, as providers that take audio
    /// inline (Vertex AI, for one) require rather than a bare format name.
    ///
    /// `Opus` reports the Ogg container it is normally carried in, and
    /// `Pcm16` reports `audio/L16`; both are outside the set Gemini
    /// documents as supported, so a request carrying them may be rejected.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mp3",
            Self::Flac => "audio/flac",
            Self::Opus => "audio/ogg",
            Self::Pcm16 => "audio/L16",
        }
    }
}
