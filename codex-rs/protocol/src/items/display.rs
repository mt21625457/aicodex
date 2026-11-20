//! The presentation instruction attached to one moderated Chat answer.
use super::AgentMessageModeration;
use schemars::JsonSchema;
use schemars::r#gen::SchemaGenerator;
use schemars::schema::Schema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use ts_rs::TS;

/// The only supported version of the answer display contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(try_from = "u8", into = "u8")]
#[ts(type = "1", export_to = "v2/")]
pub struct AgentMessageDisplayVersion;

impl TryFrom<u8> for AgentMessageDisplayVersion {
    type Error = &'static str;

    fn try_from(version: u8) -> Result<Self, Self::Error> {
        match version {
            1 => Ok(Self),
            _ => Err("unsupported answer display version"),
        }
    }
}

impl From<AgentMessageDisplayVersion> for u8 {
    fn from(_: AgentMessageDisplayVersion) -> Self {
        1
    }
}

impl JsonSchema for AgentMessageDisplayVersion {
    fn schema_name() -> String {
        "AgentMessageDisplayVersion".to_owned()
    }

    fn is_referenceable() -> bool {
        false
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = u8::json_schema(generator).into_object();
        schema.enum_values = Some(vec![Value::from(1)]);
        Schema::Object(schema)
    }
}

/// How Chat should present a recovered answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "mode", rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum AgentMessageDisplay {
    Show {
        #[ts(type = "1")]
        version: AgentMessageDisplayVersion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        annotation: Option<AnswerAnnotation>,
        #[serde(rename = "trustedContacts")]
        trusted_contacts: TrustedContacts,
    },
    Hide {
        #[ts(type = "1")]
        version: AgentMessageDisplayVersion,
        replacement: AnswerReplacement,
    },
    Block {
        #[ts(type = "1")]
        version: AgentMessageDisplayVersion,
        reason: BlockReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum AnswerAnnotation {
    WellbeingHelpline { subject: HelplineSubject },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum HelplineSubject {
    Answer,
    Prompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum TrustedContacts {
    Check,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum AnswerReplacement {
    WellbeingPromptHelpline {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum BlockReason {
    Protection {
        /// Identifies the same moderation row that Chat writes on the live path.
        #[serde(rename = "protectionType")]
        protection_type: String,
    },
    Cyber {
        #[serde(
            default,
            rename = "trustedAccess",
            skip_serializing_if = "Option::is_none"
        )]
        #[ts(optional)]
        trusted_access: Option<bool>,
    },
}

/// An unreadable instruction leaves the answer restricted.
#[derive(Deserialize)]
pub(super) struct StoredModeration {
    moderated: bool,
    display: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyAnswerDisplay {
    action: LegacyAnswerDisplayAction,
    check_trusted_contacts: Option<bool>,
    block_type: Option<String>,
    cyber_trusted_access: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum LegacyAnswerDisplayAction {
    AnswerHelpline,
}

fn read_legacy_display(value: Value) -> Option<AgentMessageDisplay> {
    let LegacyAnswerDisplay {
        action: LegacyAnswerDisplayAction::AnswerHelpline,
        check_trusted_contacts,
        block_type: None,
        cyber_trusted_access: None,
    } = serde_json::from_value(value).ok()?
    else {
        return None;
    };
    Some(AgentMessageDisplay::Show {
        version: AgentMessageDisplayVersion,
        annotation: Some(AnswerAnnotation::WellbeingHelpline {
            subject: HelplineSubject::Answer,
        }),
        trusted_contacts: if check_trusted_contacts == Some(false) {
            TrustedContacts::Skip
        } else {
            TrustedContacts::Check
        },
    })
}

impl From<StoredModeration> for AgentMessageModeration {
    fn from(stored: StoredModeration) -> Self {
        let Some(value) = stored.display else {
            return Self {
                moderated: stored.moderated,
                display: None,
            };
        };
        let display = AgentMessageDisplay::deserialize(&value)
            .ok()
            .or_else(|| read_legacy_display(value));
        Self {
            moderated: stored.moderated || display.is_none(),
            display,
        }
    }
}
