use codex_tools::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadFileArgs {
    pub(super) path: String,
    #[serde(default = "default_offset")]
    pub(super) offset: usize,
    #[serde(default = "default_limit")]
    pub(super) limit: usize,
    #[serde(default)]
    pub(super) environment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EditFileArgs {
    pub(super) path: String,
    pub(super) old_string: String,
    pub(super) new_string: String,
    #[serde(default)]
    pub(super) replace_all: bool,
    #[serde(default)]
    pub(super) environment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WriteFileArgs {
    pub(super) path: String,
    pub(super) content: String,
    #[serde(default)]
    pub(super) environment_id: Option<String>,
}

const fn default_offset() -> usize {
    1
}

const fn default_limit() -> usize {
    2_000
}

pub(super) fn file_schema<const N: usize>(
    properties: [(&str, JsonSchema); N],
    required: Vec<String>,
) -> JsonSchema {
    JsonSchema::object(
        properties
            .into_iter()
            .map(|(name, schema)| (name.to_string(), schema))
            .collect(),
        Some(required),
        Some(false.into()),
    )
}
