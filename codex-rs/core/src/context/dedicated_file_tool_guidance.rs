use super::ContextualUserFragment;

/// Bounded guidance emitted only when all dedicated file tools are callable.
pub(crate) struct DedicatedFileToolGuidance;

impl ContextualUserFragment for DedicatedFileToolGuidance {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (
            "<dedicated_file_tool_guidance>",
            "</dedicated_file_tool_guidance>",
        )
    }

    fn body(&self) -> String {
        "\nUse read_file for ordinary text reads, edit_file for exact edits after a read in an earlier completion, and write_file for creates or whole-file writes. Keep dependent reads and mutations in separate completions. Use shell or a specialized script only for binary files, unsupported text encodings, or files above the editable limit.\n".to_string()
    }
}
