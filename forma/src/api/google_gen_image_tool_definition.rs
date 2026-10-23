use agent::tool::google_gen_image_tool::{
    GoogleGenImageTool, default_gen_image_location, default_gen_image_model,
    default_gen_image_timeout_seconds,
};
use derive::FormaDocs;
use genai_client::vertex_ai::{HarmBlockThreshold, HarmCategory, SafetySetting};
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::ObjectDefinition;

/// Which of Google's harm filters a setting applies to.
///
/// A Forma-side mirror of the Vertex categories, so that an agent file can
/// name them in kebab-case like everything else it configures.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, FormaDocs)]
#[serde(rename_all = "kebab-case")]
pub enum HarmCategoryDefinition {
    HateSpeech,
    DangerousContent,
    SexuallyExplicit,
    Harassment,
}

impl From<HarmCategoryDefinition> for HarmCategory {
    fn from(value: HarmCategoryDefinition) -> Self {
        match value {
            HarmCategoryDefinition::HateSpeech => HarmCategory::HateSpeech,
            HarmCategoryDefinition::DangerousContent => HarmCategory::DangerousContent,
            HarmCategoryDefinition::SexuallyExplicit => HarmCategory::SexuallyExplicit,
            HarmCategoryDefinition::Harassment => HarmCategory::Harassment,
        }
    }
}

/// How much to block in a given category.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, FormaDocs)]
#[serde(rename_all = "kebab-case")]
pub enum HarmThresholdDefinition {
    /// Turn the filter off entirely. This is Forma's default.
    #[default]
    Off,
    /// Block nothing, but still score the content.
    BlockNone,
    /// Block only what scores as high risk.
    BlockOnlyHigh,
    /// Block medium risk and above.
    BlockMediumAndAbove,
    /// Block low risk and above — the strictest setting.
    BlockLowAndAbove,
}

impl From<HarmThresholdDefinition> for HarmBlockThreshold {
    fn from(value: HarmThresholdDefinition) -> Self {
        match value {
            HarmThresholdDefinition::Off => HarmBlockThreshold::Off,
            HarmThresholdDefinition::BlockNone => HarmBlockThreshold::BlockNone,
            HarmThresholdDefinition::BlockOnlyHigh => HarmBlockThreshold::BlockOnlyHigh,
            HarmThresholdDefinition::BlockMediumAndAbove => HarmBlockThreshold::BlockMediumAndAbove,
            HarmThresholdDefinition::BlockLowAndAbove => HarmBlockThreshold::BlockLowAndAbove,
        }
    }
}

/// One category's filter setting.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, FormaDocs)]
pub struct SafetySettingDefinition {
    /// The harm category this applies to.
    pub category: HarmCategoryDefinition,
    /// How much of that category to block. Defaults to `off`.
    #[serde(default)]
    pub threshold: HarmThresholdDefinition,
}

impl From<SafetySettingDefinition> for SafetySetting {
    fn from(value: SafetySettingDefinition) -> Self {
        SafetySetting::new()
            .set_category(HarmCategory::from(value.category))
            .set_threshold(HarmBlockThreshold::from(value.threshold))
    }
}

/// Generates images with a Gemini image model on Vertex AI.
///
/// The picture is streamed to the client as it is produced; the LLM is told
/// only that the generation succeeded, so the bytes never enter its context
/// and never cost tokens. Ask for one image per call.
///
/// Authentication uses Google Cloud application-default credentials, which are
/// injected at call time — no API key env var is required. The only argument
/// the LLM supplies is the `prompt`.
///
/// > **NOTE**: the caller needs `roles/aiplatform.user` on the project, and
/// > the model must be one that can return images.
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
pub struct GoogleGenImageDefinition {
    /// The Google Cloud project billed for the generation.
    ///
    /// Optional: when it is not given, the project named by the
    /// `GOOGLE_CLOUD_PROJECT` environment variable of whichever machine runs
    /// the agent is billed instead, resolved when the tool is called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,

    /// The image-capable Gemini model to call.
    ///
    /// Examples are:
    /// - `gemini-3.1-flash-lite-image` (default, aka. Nano banana 2 lite)
    /// - `gemini-3.1-flash-image` (aka., Nano banana 2)
    /// - `gemini-3-pro-image` (aka., Nano banana pro)
    /// - `gemini-2.5-flash-image` (aka., Nano banana)
    #[serde(default = "default_gen_image_model")]
    pub model: String,

    /// The Vertex location. Defaults to `global`; a region such as
    /// `europe-west4` uses its own regional host.
    #[serde(default = "default_gen_image_location")]
    pub location: String,

    /// The shape of the generated image — `1:1`, `16:9`, `9:16`, `4:3`,
    /// `3:4`, `21:9` and so on. When omitted the model picks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,

    /// Google's harm filters.
    ///
    /// Every configurable filter is **off** unless named here: Forma does not
    /// second-guess what an agent author wants blocked, so opting into
    /// Google's filters is an explicit act. Listing a category overrides that
    /// one; the rest stay off.
    #[serde(default)]
    pub safety_settings: Vec<SafetySettingDefinition>,

    /// How long a single generation may take, in seconds. Images are slower
    /// than text, so this is well above a chat timeout.
    #[serde(default = "default_gen_image_timeout_seconds")]
    pub request_timeout_seconds: u64,
}

impl Default for GoogleGenImageDefinition {
    fn default() -> Self {
        Self {
            project_id: None,
            model: default_gen_image_model(),
            location: default_gen_image_location(),
            aspect_ratio: None,
            safety_settings: Vec::new(),
            request_timeout_seconds: default_gen_image_timeout_seconds(),
        }
    }
}

impl ObjectDefinition for GoogleGenImageDefinition {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

impl TryFrom<GoogleGenImageDefinition> for GoogleGenImageTool {
    type Error = FormaError;

    fn try_from(value: GoogleGenImageDefinition) -> Result<Self, Self::Error> {
        // Off is the per-category default, not merely the whole-list one, so
        // the stated settings are layered over an all-off baseline. Sending
        // only what was listed would silently hand the unlisted categories
        // back to Google's own defaults, which is the opposite of what
        // configuring one filter should mean.
        let mut safety_settings = genai_client::vertex_ai::safety_settings_off();
        for stated in value.safety_settings {
            let stated: SafetySetting = stated.into();
            match safety_settings
                .iter_mut()
                .find(|s| s.category == stated.category)
            {
                Some(existing) => *existing = stated,
                None => safety_settings.push(stated),
            }
        }

        // The project is passed through as-is, unresolved: reading the
        // environment here would tie the compiled agent to the machine that
        // compiled it. The tool asks for it when it is actually called.
        Ok(GoogleGenImageTool {
            project_id: value.project_id,
            model: value.model,
            location: value.location,
            aspect_ratio: value.aspect_ratio,
            safety_settings,
            request_timeout_seconds: value.request_timeout_seconds,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ProjectEnvGuard;
    use agent::ToolTrait;

    // ── the project ───────────────────────────────────────────────────────

    /// The whole point: an agent file that names no project is still a valid
    /// agent file on a machine that has no project set. Nothing in the load
    /// path may reach for the environment — the tool does that when it runs.
    #[test]
    fn a_definition_without_a_project_needs_no_environment() {
        let _env = ProjectEnvGuard::set(None);

        let def: GoogleGenImageDefinition = serde_yaml::from_str("aspect_ratio: \"16:9\"")
            .expect("a definition with no project must still parse");
        assert_eq!(def.project_id, None);

        let tool: GoogleGenImageTool = def.try_into().expect("converts without an environment");
        assert_eq!(
            tool.project_id, None,
            "the project must stay unresolved until the tool is called"
        );
        assert!(
            <GoogleGenImageTool as ToolTrait<db::NoneDB>>::check(&tool).is_ok(),
            "a project supplied at run time is not a load-time error"
        );
    }

    /// Present in the environment but absent from the file: the definition
    /// must still not bake it in, or the compiled agent would carry whichever
    /// project happened to be set on the machine that compiled it.
    #[test]
    fn the_environment_does_not_leak_into_the_definition() {
        let _env = ProjectEnvGuard::set(Some("env-project"));

        let def: GoogleGenImageDefinition =
            serde_yaml::from_str("model: some-model").expect("parses");
        assert_eq!(def.project_id, None);

        let tool: GoogleGenImageTool = def.try_into().expect("converts");
        assert_eq!(tool.project_id, None);
    }

    /// A project the author did write is carried through untouched, whatever
    /// the environment says.
    #[test]
    fn a_stated_project_survives_the_conversion() {
        let _env = ProjectEnvGuard::set(Some("env-project"));

        let def: GoogleGenImageDefinition =
            serde_yaml::from_str("project_id: some-project").expect("parses");
        let tool: GoogleGenImageTool = def.try_into().expect("converts");
        assert_eq!(tool.project_id.as_deref(), Some("some-project"));
    }

    #[test]
    fn an_unstated_project_is_not_serialised_back_out() {
        let def = GoogleGenImageDefinition::default();
        let yaml = serde_yaml::to_string(&def).expect("serialises");
        assert!(
            !yaml.contains("project_id"),
            "an absent project must not reappear on the way out: {yaml}"
        );
    }

    // ── safety settings ───────────────────────────────────────────────────

    #[test]
    fn an_unstated_list_turns_every_filter_off() {
        let def = GoogleGenImageDefinition {
            project_id: Some("p".to_string()),
            ..Default::default()
        };
        let tool: GoogleGenImageTool = def.try_into().expect("converts");
        assert_eq!(tool.safety_settings.len(), 4);
        for s in &tool.safety_settings {
            assert_eq!(s.threshold.name(), Some("OFF"));
        }
    }

    #[test]
    fn a_stated_list_is_used_as_written() {
        let def = GoogleGenImageDefinition {
            project_id: Some("p".to_string()),
            safety_settings: vec![SafetySettingDefinition {
                category: HarmCategoryDefinition::DangerousContent,
                threshold: HarmThresholdDefinition::BlockOnlyHigh,
            }],
            ..Default::default()
        };
        // The stated category is applied, and the three unstated ones stay
        // off rather than reverting to Google's defaults.
        let tool: GoogleGenImageTool = def.try_into().expect("converts");
        assert_eq!(tool.safety_settings.len(), 4);

        let stated = tool
            .safety_settings
            .iter()
            .find(|s| s.category.name() == Some("HARM_CATEGORY_DANGEROUS_CONTENT"))
            .expect("the stated category");
        assert_eq!(stated.threshold.name(), Some("BLOCK_ONLY_HIGH"));

        for other in tool
            .safety_settings
            .iter()
            .filter(|s| s.category.name() != Some("HARM_CATEGORY_DANGEROUS_CONTENT"))
        {
            assert_eq!(
                other.threshold.name(),
                Some("OFF"),
                "an unstated category must stay off"
            );
        }
    }

    #[test]
    fn a_setting_defaults_to_off_when_only_a_category_is_given() {
        let def: SafetySettingDefinition =
            serde_yaml::from_str("category: hate-speech").expect("parses");
        assert_eq!(
            HarmBlockThreshold::from(def.threshold).name(),
            Some("OFF"),
            "omitting the threshold must mean off, as documented"
        );
    }

    #[test]
    fn the_yaml_an_agent_author_writes_round_trips() {
        let def: GoogleGenImageDefinition = serde_yaml::from_str(
            r#"
project_id: my-project
aspect_ratio: "16:9"
safety_settings:
  - category: sexually-explicit
    threshold: block-medium-and-above
"#,
        )
        .expect("parses");

        assert_eq!(def.project_id, Some("my-project".to_string()));
        assert_eq!(def.aspect_ratio.as_deref(), Some("16:9"));
        // Defaults fill in the rest.
        assert_eq!(def.model, default_gen_image_model());
        assert_eq!(def.location, "global");

        let tool: GoogleGenImageTool = def.try_into().expect("converts");
        assert!(<GoogleGenImageTool as ToolTrait<db::NoneDB>>::check(&tool).is_ok());

        let explicit = tool
            .safety_settings
            .iter()
            .find(|s| s.category.name() == Some("HARM_CATEGORY_SEXUALLY_EXPLICIT"))
            .expect("the stated category");
        assert_eq!(explicit.threshold.name(), Some("BLOCK_MEDIUM_AND_ABOVE"));
    }
}
