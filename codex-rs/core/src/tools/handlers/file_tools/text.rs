use codex_apply_patch::PatchableTextEncoding;

pub(super) struct DecodedText {
    pub(super) text: String,
    pub(super) encoding: PatchableTextEncoding,
    pub(super) had_crlf: bool,
    pub(super) utf8_bom: bool,
}

pub(super) fn decode_text(bytes: Vec<u8>) -> std::io::Result<DecodedText> {
    let utf8_bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let decoded = codex_apply_patch::decode_patchable_text(bytes)?;
    let had_crlf = decoded.contents.contains("\r\n");
    let contents = decoded
        .contents
        .strip_prefix('\u{feff}')
        .unwrap_or(&decoded.contents);
    Ok(DecodedText {
        text: contents.replace("\r\n", "\n"),
        encoding: decoded.encoding,
        had_crlf,
        utf8_bom,
    })
}

pub(super) fn decode_scanned_text(
    mut bytes: Vec<u8>,
    reached_eof: bool,
) -> std::io::Result<DecodedText> {
    if !reached_eof {
        let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the bounded scan did not reach a complete text line",
            ));
        };
        bytes.truncate(last_newline + 1);
    }
    decode_text(bytes)
}

pub(super) fn encode_text(
    text: &str,
    encoding: PatchableTextEncoding,
    had_crlf: bool,
    utf8_bom: bool,
) -> std::io::Result<Vec<u8>> {
    let text = if had_crlf {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text.to_string()
    };
    let mut bytes = encoding.encode(&text)?;
    if utf8_bom && matches!(encoding, PatchableTextEncoding::Utf8) {
        bytes.splice(0..0, [0xEF, 0xBB, 0xBF]);
    }
    Ok(bytes)
}
