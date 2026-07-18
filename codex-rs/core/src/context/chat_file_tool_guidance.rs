use super::ContextualUserFragment;

/// Bounded guidance for Chat Completions sessions exposing dedicated file tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatFileToolGuidance {
    read_name: String,
    edit_name: String,
    write_name: String,
}

impl ChatFileToolGuidance {
    pub(crate) fn new(
        read_name: impl Into<String>,
        edit_name: impl Into<String>,
        write_name: impl Into<String>,
    ) -> Self {
        Self {
            read_name: read_name.into(),
            edit_name: edit_name.into(),
            write_name: write_name.into(),
        }
    }
}

impl ContextualUserFragment for ChatFileToolGuidance {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<chat_file_tool_guidance>", "</chat_file_tool_guidance>")
    }

    fn body(&self) -> String {
        format!(
            "\nUse the callable file tools for ordinary text file IO. Read with `{}`; edit with `{}` only after a read in an earlier completion; create or overwrite with `{}` only after the required read. Keep dependent Read→Edit/Write calls in separate completions. Use shell or a script only when a file tool reports binary, unsupported encoding, or an editable-size limit.\n",
            self.read_name, self.edit_name, self.write_name
        )
    }
}
