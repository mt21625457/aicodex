use std::io;

use chardetng::EncodingDetector;
use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::FileSystemSandboxContext;
use codex_exec_server::ReadFileOptions;
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

/// Decoded text plus the encoding required to write it back without format drift.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchableTextFile {
    pub contents: String,
    pub encoding: PatchableTextEncoding,
}

/// Text encoding accepted by the shared patchable-file decoder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PatchableTextEncoding {
    Utf8,
    Legacy(&'static Encoding),
}

impl PatchableTextEncoding {
    /// Encodes updated contents using the original file encoding.
    pub fn encode(self, contents: &str) -> io::Result<Vec<u8>> {
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
    path: &PathUri,
    fs: &dyn ExecutorFileSystem,
    follow_symlinks: bool,
    sandbox: Option<&FileSystemSandboxContext>,
) -> io::Result<PatchableTextFile> {
    let bytes = fs
        .read_file(path, ReadFileOptions { follow_symlinks }, sandbox)
        .await?;
    decode_patchable_text(bytes)
}

/// Decodes UTF-8 or a supported round-trippable legacy text encoding.
pub fn decode_patchable_text(bytes: Vec<u8>) -> io::Result<PatchableTextFile> {
    match String::from_utf8(bytes) {
        Ok(contents) if looks_like_plain_text(&contents) => Ok(PatchableTextFile {
            contents,
            encoding: PatchableTextEncoding::Utf8,
        }),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "UTF-8 file contains binary control characters",
        )),
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
    fn utf8_with_binary_control_characters_is_rejected() {
        let error = decode_patchable_text(b"text\0binary".to_vec())
            .expect_err("NUL-containing UTF-8 must not be treated as patchable text");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
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
pub(super) type Replacement = (usize, usize, Vec<String>);

#[derive(Clone, Copy)]
enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
            Self::Cr => "\r",
        }
    }
}

struct SourceLine {
    text: String,
    ending: Option<LineEnding>,
}

pub(super) struct SourceFile {
    lines: Vec<SourceLine>,
    preferred_ending: LineEnding,
}

impl SourceFile {
    /// Splits contents into logical lines while retaining each line ending.
    ///
    /// The first existing ending becomes the preferred style for inserted
    /// lines; files without an ending default to LF.
    pub(super) fn parse(contents: &str) -> Self {
        let mut lines = Vec::new();
        let mut preferred_ending = None;
        let mut line_start = 0;
        let mut cursor = 0;

        while cursor < contents.len() {
            let (ending, ending_len) = match contents.as_bytes()[cursor] {
                b'\r' if contents.as_bytes().get(cursor + 1) == Some(&b'\n') => {
                    (LineEnding::CrLf, 2)
                }
                b'\r' => (LineEnding::Cr, 1),
                b'\n' => (LineEnding::Lf, 1),
                _ => {
                    cursor += 1;
                    continue;
                }
            };
            preferred_ending.get_or_insert(ending);
            lines.push(SourceLine {
                text: contents[line_start..cursor].to_string(),
                ending: Some(ending),
            });
            cursor += ending_len;
            line_start = cursor;
        }

        if line_start < contents.len() {
            lines.push(SourceLine {
                text: contents[line_start..].to_string(),
                ending: None,
            });
        }

        Self {
            lines,
            preferred_ending: preferred_ending.unwrap_or(LineEnding::Lf),
        }
    }

    pub(super) fn line_texts(&self) -> Vec<String> {
        self.lines.iter().map(|line| line.text.clone()).collect()
    }

    /// Rebuilds the file from source-ordered, non-overlapping replacements.
    ///
    /// Unchanged lines retain their original endings, inserted lines use the
    /// preferred ending, and every resulting line receives an ending to match
    /// apply-patch's historical trailing-newline behavior.
    pub(super) fn apply_replacements(&mut self, replacements: &[Replacement]) {
        let mut source_lines = std::mem::take(&mut self.lines).into_iter();
        let mut new_lines = Vec::new();
        let mut source_index = 0;

        for (start_idx, old_len, new_segment) in replacements {
            debug_assert!(*start_idx >= source_index);
            for line in source_lines.by_ref().take(*start_idx - source_index) {
                new_lines.push(line);
            }
            for _ in source_lines.by_ref().take(*old_len) {}
            new_lines.extend(new_segment.iter().map(|text| SourceLine {
                text: text.clone(),
                ending: Some(self.preferred_ending),
            }));
            source_index = start_idx + old_len;
        }
        new_lines.extend(source_lines);
        self.lines = new_lines;

        // Updates have historically added a trailing newline. This also gives
        // an unterminated last line an ending if an insertion moved it inward.
        for line in &mut self.lines {
            line.ending.get_or_insert(self.preferred_ending);
        }
    }

    pub(super) fn into_contents(self) -> String {
        let mut contents = String::new();
        for line in self.lines {
            contents.push_str(&line.text);
            if let Some(ending) = line.ending {
                contents.push_str(ending.as_str());
            }
        }
        contents
    }
}
