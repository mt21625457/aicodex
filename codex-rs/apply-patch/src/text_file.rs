use std::io;

use chardetng::EncodingDetector;
use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::FileSystemSandboxContext;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use encoding_rs::EUC_KR;
use encoding_rs::Encoding;
use encoding_rs::GB18030;
use encoding_rs::GBK;
#[cfg(test)]
use encoding_rs::SHIFT_JIS;
use encoding_rs::UTF_8;
use encoding_rs::UTF_16BE;
use encoding_rs::UTF_16LE;
use encoding_rs::WINDOWS_1252;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PatchableTextFile {
    pub(crate) contents: String,
    pub(crate) encoding: PatchableTextEncoding,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PatchableTextEncoding {
    Utf8,
    Legacy(&'static Encoding),
}

impl PatchableTextEncoding {
    pub(crate) fn encode(self, contents: &str) -> io::Result<Vec<u8>> {
        match self {
            PatchableTextEncoding::Utf8 => Ok(contents.as_bytes().to_vec()),
            PatchableTextEncoding::Legacy(encoding) => {
                let (encoded, _, had_errors) = encoding.encode(contents);
                if had_errors {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "updated contents contain characters that cannot be represented as {}",
                            encoding.name()
                        ),
                    ));
                }
                Ok(encoded.into_owned())
            }
        }
    }
}

pub(crate) async fn read_patchable_text_file(
    path: &AbsolutePathBuf,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> io::Result<PatchableTextFile> {
    let path_uri = PathUri::from_abs_path(path);
    let bytes = fs.read_file(&path_uri, sandbox).await?;
    decode_patchable_text(bytes)
}

fn decode_patchable_text(bytes: Vec<u8>) -> io::Result<PatchableTextFile> {
    match String::from_utf8(bytes) {
        Ok(contents) => Ok(PatchableTextFile {
            contents,
            encoding: PatchableTextEncoding::Utf8,
        }),
        Err(error) => decode_legacy_patchable_text(error.into_bytes()),
    }
}

fn decode_legacy_patchable_text(bytes: Vec<u8>) -> io::Result<PatchableTextFile> {
    let mut detector = EncodingDetector::new();
    detector.feed(&bytes, true);
    let (detected, is_confident) = detector.guess_assess(None, true);

    let mut candidates = Vec::new();
    let prefer_gbk = should_prefer_gbk_over_detected(detected, &bytes);

    if is_confident && !prefer_gbk {
        add_candidate(&mut candidates, detected);
    }
    add_candidate(&mut candidates, GBK);
    add_candidate(&mut candidates, GB18030);
    if !is_confident || prefer_gbk {
        add_candidate(&mut candidates, detected);
    }
    add_candidate(&mut candidates, WINDOWS_1252);

    for encoding in candidates {
        if encoding.is_single_byte() && !looks_like_windows_1252_text(&bytes) {
            continue;
        }
        if encoding == detected
            && !is_confident
            && !looks_like_windows_1252_text(&bytes)
            && encoding != GBK
            && encoding != GB18030
        {
            continue;
        }
        if encoding == WINDOWS_1252 && !looks_like_windows_1252_text(&bytes) {
            continue;
        }
        let Some(decoded) = encoding.decode_without_bom_handling_and_without_replacement(&bytes)
        else {
            continue;
        };
        let contents = decoded.into_owned();
        if !looks_like_plain_text(&contents) {
            continue;
        }
        if !legacy_encoding_round_trips(encoding, &contents, &bytes) {
            continue;
        }
        return Ok(PatchableTextFile {
            contents,
            encoding: PatchableTextEncoding::Legacy(encoding),
        });
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "file is not valid UTF-8 and no supported legacy text encoding matched",
    ))
}

fn add_candidate(candidates: &mut Vec<&'static Encoding>, encoding: &'static Encoding) {
    if is_supported_legacy_encoding(encoding) && !candidates.contains(&encoding) {
        candidates.push(encoding);
    }
}

fn should_prefer_gbk_over_detected(detected: &'static Encoding, bytes: &[u8]) -> bool {
    if detected != EUC_KR {
        return false;
    }

    let Some(decoded) = GBK.decode_without_bom_handling_and_without_replacement(bytes) else {
        return false;
    };
    let contents = decoded.into_owned();
    looks_like_plain_text(&contents)
        && contains_han_character(&contents)
        && legacy_encoding_round_trips(GBK, &contents, bytes)
}

fn is_supported_legacy_encoding(encoding: &'static Encoding) -> bool {
    encoding != UTF_8 && encoding != UTF_16LE && encoding != UTF_16BE
}

fn legacy_encoding_round_trips(
    encoding: &'static Encoding,
    contents: &str,
    original_bytes: &[u8],
) -> bool {
    let (encoded, _, had_errors) = encoding.encode(contents);
    !had_errors && encoded.as_ref() == original_bytes
}

fn looks_like_windows_1252_text(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|byte| matches!(byte, b'\n' | b'\r' | b'\t' | 0x20..=0x7e))
}

fn looks_like_plain_text(contents: &str) -> bool {
    contents
        .chars()
        .all(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t' | '\x0c'))
}

fn contains_han_character(contents: &str) -> bool {
    contents.chars().any(is_han_character)
}

fn is_han_character(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{20000}'..='\u{2a6df}'
            | '\u{2a700}'..='\u{2b73f}'
            | '\u{2b740}'..='\u{2b81f}'
            | '\u{2b820}'..='\u{2ceaf}'
            | '\u{2ceb0}'..='\u{2ebef}'
            | '\u{30000}'..='\u{3134f}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_text_round_trips_after_ascii_edit() {
        let file = decode_patchable_text("// 你好 🙂\nint value = 1;\n".as_bytes().to_vec())
            .expect("UTF-8 text should decode");

        assert_eq!(file.contents, "// 你好 🙂\nint value = 1;\n");
        assert_eq!(file.encoding, PatchableTextEncoding::Utf8);

        let updated = file
            .encoding
            .encode("// 你好 🙂\nint value = 2;\n")
            .expect("updated text should encode as UTF-8");

        assert_eq!(updated, "// 你好 🙂\nint value = 2;\n".as_bytes());
    }

    #[test]
    fn confident_legacy_detection_is_preferred_before_gbk_fallback() {
        let contents = "// こんにちは世界\n// これは日本語のコメントです。\nint value = 1;\n";
        let (bytes, _, had_errors) = SHIFT_JIS.encode(contents);
        assert!(!had_errors);
        let bytes = bytes.into_owned();
        let mut detector = EncodingDetector::new();
        detector.feed(&bytes, true);
        let (detected, is_confident) = detector.guess_assess(None, true);
        assert_eq!(detected, SHIFT_JIS);
        assert!(is_confident);

        let file = decode_patchable_text(bytes).expect("Shift_JIS text should decode");

        assert_eq!(
            file,
            PatchableTextFile {
                contents: contents.to_string(),
                encoding: PatchableTextEncoding::Legacy(SHIFT_JIS),
            }
        );
    }

    #[test]
    fn gbk_text_round_trips_after_ascii_edit() {
        let bytes = b"// \xc4\xe3\xba\xc3\nint value = 1;\n".to_vec();
        let file = decode_patchable_text(bytes).expect("GBK text should decode");

        assert_eq!(file.contents, "// \u{4f60}\u{597d}\nint value = 1;\n");

        let updated = file
            .encoding
            .encode("// \u{4f60}\u{597d}\nint value = 2;\n")
            .expect("updated text should encode as GBK");

        assert_eq!(updated, b"// \xc4\xe3\xba\xc3\nint value = 2;\n");
    }

    #[test]
    fn legacy_encoding_rejects_unrepresentable_updates() {
        let bytes = b"// \xc4\xe3\xba\xc3\n".to_vec();
        let file = decode_patchable_text(bytes).expect("GBK text should decode");
        let error = file
            .encoding
            .encode("// \u{4f60}\u{597d} \u{1f642}\n")
            .expect_err("emoji cannot be encoded as GBK");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn binary_bytes_are_not_treated_as_patchable_text() {
        let error =
            decode_patchable_text(vec![0xff, 0xfe, 0xfd]).expect_err("binary should not decode");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
