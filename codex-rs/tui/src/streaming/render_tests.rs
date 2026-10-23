use super::StreamingRender;
use super::render_source;
use crate::history_cell::HistoryRenderMode;
use crate::inline_visualization::InlineVisualizationContext;
use crate::markdown::render_streaming_markdown_agent_with_links_and_cwd;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::visible_lines;
use codex_protocol::ThreadId;
use insta::assert_debug_snapshot;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::path::PathBuf;

#[path = "code_fence_render_tests.rs"]
mod code_fence_tests;

fn test_cwd() -> PathBuf {
    std::env::temp_dir()
}

fn append(
    render: &mut StreamingRender,
    source: &mut String,
    chunk: &str,
    width: Option<usize>,
    cwd: &Path,
    render_mode: HistoryRenderMode,
) {
    source.push_str(chunk);
    render.append(
        source,
        chunk,
        width,
        cwd,
        render_mode,
        /*inline_visualization_context*/ None,
    );
}

fn append_rich_and_assert_matches_full(
    render: &mut StreamingRender,
    source: &mut String,
    chunk: &str,
    width: Option<usize>,
    cwd: &Path,
) {
    append(render, source, chunk, width, cwd, HistoryRenderMode::Rich);
    assert_eq!(
        render.lines,
        render_source(
            source,
            width,
            cwd,
            HistoryRenderMode::Rich,
            /*inline_visualization_context*/ None,
        ),
        "incremental render diverged after chunk {chunk:?}",
    );
}

fn lines_to_plain_strings(lines: &[HyperlinkLine]) -> Vec<String> {
    visible_lines(lines.to_vec())
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn assert_rich_stream_matches_full_render(
    chunks: &[&str],
    width: Option<usize>,
) -> (String, StreamingRender) {
    let cwd = test_cwd();
    let mut source = String::new();
    let mut render = StreamingRender::new();

    for chunk in chunks {
        append_rich_and_assert_matches_full(&mut render, &mut source, chunk, width, &cwd);
    }

    (source, render)
}

#[test]
fn incremental_render_keeps_final_block_mutable_and_matches_full_render() {
    let chunks = [
        "# Heading\n",
        "\n",
        "First paragraph with a [link](https://example.com).\n",
        "continued on the next line.\n\n",
        "1. First item\n",
        "2. Second item\n\n",
        "> Quoted paragraph\n\n",
        "```rust\n",
        "fn main() {}\n",
        "```\n\n",
        "| Key | Value |\n",
        "| --- | --- |\n",
        "| alpha | beta |\n",
    ];
    let (source, render) = crate::terminal_palette::with_test_default_colors(
        crate::terminal_probe::DefaultColors {
            fg: (220, 220, 220),
            bg: (20, 20, 20),
        },
        || assert_rich_stream_matches_full_render(&chunks, /*width*/ Some(48)),
    );

    assert!(render.stable_source_len > 0);
    assert!(render.stable_source_len < source.len());
    assert_debug_snapshot!("incremental_render_representative_stream", render.lines);
}

#[test]
fn incremental_file_citations_preserve_metadata_unicode_and_markdown() {
    let cwd = test_cwd();
    let rendered_cases = [
        (
            "Quarterly Report.xlsx",
            "- :codex-file-citation{artifact_kind=\"workbook\" ",
        ),
        ("Résumé *final* ✨.xlsx", "- :codex-file-citation{"),
    ]
    .map(|(filename, prefix)| {
        let tail = format!("path=\"{}\"}}\n", cwd.join(filename).display());
        let chunks = ["# Output\n\n", prefix, &tail, "\n", "Continue.\n"];
        let (_, render) = crate::terminal_palette::with_test_default_colors(
            crate::terminal_probe::DefaultColors {
                fg: (220, 220, 220),
                bg: (20, 20, 20),
            },
            || assert_rich_stream_matches_full_render(&chunks, /*width*/ Some(80)),
        );

        render.lines
    });
    assert_debug_snapshot!("incremental_file_citations", rendered_cases);
}

#[test]
fn growing_single_top_level_blocks_render_and_scan_in_one_pass() {
    let streams: &[&[&str]] = &[
        &[
            "A paragraph that keeps growing\n",
            "without a blank line between chunks.\n",
            "It stays one top-level block.\n",
        ],
        &[
            "| Key | Value |\n",
            "| --- | --- |\n",
            "| alpha | beta |\n",
            "| gamma | delta |\n",
        ],
    ];
    let cwd = test_cwd();
    let width = Some(80);

    for chunks in streams {
        let mut source = String::new();
        let mut render = StreamingRender::new();
        for chunk in *chunks {
            append_rich_and_assert_matches_full(&mut render, &mut source, chunk, width, &cwd);
            let pending = render_streaming_markdown_agent_with_links_and_cwd(
                &source,
                width,
                Some(cwd.as_path()),
                crate::markdown_render::ListSpacing::AfterMultiline,
            );
            assert_eq!(pending.last_top_level_block_start, None);
            assert_eq!(render.lines, pending.lines);
            assert_eq!(render.stable_source_len, 0);
        }
    }
}

#[test]
fn incremental_raw_render_preserves_blank_lines() {
    let cwd = test_cwd();
    let width = Some(80);
    let mut source = String::new();
    let mut render = StreamingRender::new();

    for chunk in ["alpha\n", "\n", "beta\n", "\n"] {
        append(
            &mut render,
            &mut source,
            chunk,
            width,
            &cwd,
            HistoryRenderMode::Raw,
        );
    }

    assert_eq!(
        lines_to_plain_strings(&render.lines),
        vec!["alpha", "", "beta", ""],
    );
}

#[test]
fn compact_list_spacing_keeps_incremental_and_full_renders_aligned() {
    use crate::markdown_render::ListSpacing;

    let cwd = test_cwd();
    for mode in [HistoryRenderMode::Rich, HistoryRenderMode::Raw] {
        let mut render = StreamingRender::new();
        render.list_spacing = ListSpacing::Compact;
        let mut source = String::new();
        for chunk in [
            "- Short\n",
            "- A sibling with enough words to wrap across rows\n",
            "- Last\n",
            "\nAfter the list.\n\n",
            "| Name | Value |\n| --- | --- |\n| A | B |\n",
            "\n- Nested\n  - One\n  - Two\n- End\n",
        ] {
            append(&mut render, &mut source, chunk, Some(24), &cwd, mode);
            assert_eq!(
                render.lines,
                super::render_source_with_list_spacing(
                    &source,
                    Some(24),
                    &cwd,
                    mode,
                    /*inline_visualization_context*/ None,
                    ListSpacing::Compact,
                )
            );
        }
        for width in [80, 24] {
            render.recompute(
                &source,
                Some(width),
                &cwd,
                mode,
                /*inline_visualization_context*/ None,
            );
            assert_eq!(
                render.lines,
                super::render_source_with_list_spacing(
                    &source,
                    Some(width),
                    &cwd,
                    mode,
                    /*inline_visualization_context*/ None,
                    ListSpacing::Compact,
                )
            );
        }
    }
}

#[test]
fn inline_visualization_context_without_directives_keeps_stable_prefix() {
    let cwd = test_cwd();
    let context = InlineVisualizationContext::new(&cwd, ThreadId::new())
        .expect("UUIDv7 thread id should provide a timestamp");
    let width = Some(80);
    let mut source = String::new();
    let mut render = StreamingRender::new();

    for chunk in ["First paragraph.\n\n", "Second paragraph.\n\n"] {
        source.push_str(chunk);
        render.append(
            &source,
            chunk,
            width,
            &cwd,
            HistoryRenderMode::Rich,
            Some(&context),
        );
        assert_eq!(
            render.lines,
            render_source(
                &source,
                width,
                &cwd,
                HistoryRenderMode::Rich,
                Some(&context),
            ),
        );
    }

    assert!(render.stable_source_len > 0);
    assert!(!render.has_inline_visualization_directive);
}

#[test]
fn inline_visualizations_use_canonical_full_render() {
    let cwd = test_cwd();
    let context = InlineVisualizationContext::new(&cwd, ThreadId::new())
        .expect("UUIDv7 thread id should provide a timestamp");
    let width = Some(80);
    let mut source = String::new();
    let mut render = StreamingRender::new();

    for chunk in ["Before.\n\n", "::codex-inline-vis{file=\"missing.html\"}\n"] {
        source.push_str(chunk);
        render.append(
            &source,
            chunk,
            width,
            &cwd,
            HistoryRenderMode::Rich,
            Some(&context),
        );
        assert_eq!(
            render.lines,
            render_source(
                &source,
                width,
                &cwd,
                HistoryRenderMode::Rich,
                Some(&context),
            ),
        );
        assert_eq!(render.stable_source_len, 0);
    }

    assert!(render.has_inline_visualization_directive);
}

#[test]
fn inline_visualizations_without_context_use_canonical_full_render() {
    let (_, render) = assert_rich_stream_matches_full_render(
        &["Before.\n\n", "::codex-inline-vis{file=\"missing.html\"}\n"],
        Some(80),
    );

    assert_eq!(render.stable_source_len, 0);
    assert!(render.has_inline_visualization_directive);
    assert_debug_snapshot!(
        "inline_visualizations_without_context_use_canonical_full_render",
        render.lines
    );
}

#[test]
fn inline_visualization_content_references_use_canonical_full_render() {
    let (_, render) = assert_rich_stream_matches_full_render(
        &[
            "Before.\n\n",
            "\u{e200}visualize\u{e202}{\"path\":\"/tmp/missing.html\"}\u{e201}\n",
        ],
        Some(80),
    );

    assert_eq!(render.stable_source_len, 0);
    assert!(render.has_inline_visualization_directive);
}

#[test]
fn inline_visualization_directive_survives_raw_to_rich_render_mode_switch() {
    let cwd = test_cwd();
    let width = Some(80);
    let mut source = String::new();
    let mut render = StreamingRender::new();

    append(
        &mut render,
        &mut source,
        "::codex-inline-vis{file=\"missing.html\"}\n",
        width,
        &cwd,
        HistoryRenderMode::Raw,
    );
    assert!(!render.has_inline_visualization_directive);

    render.recompute(
        &source,
        width,
        &cwd,
        HistoryRenderMode::Rich,
        /*inline_visualization_context*/ None,
    );

    assert!(render.has_inline_visualization_directive);
    assert_eq!(
        render.lines,
        render_source(
            &source,
            width,
            &cwd,
            HistoryRenderMode::Rich,
            /*inline_visualization_context*/ None,
        ),
    );

    render.clear();
    assert!(!render.has_inline_visualization_directive);
}

#[test]
fn reference_link_definition_recomputes_earlier_and_later_blocks() {
    let streams: &[&[&str]] = &[
        &[
            "Earlier [reference][id].\n\n",
            "An unrelated paragraph.\n\n",
            "[id]: https://example.com/reference\n",
            "\n",
            "Later [reference][id].\n",
        ],
        &[
            "Earlier [reference][id].\n\n",
            "Another paragraph.\n\n",
            "```markdown\n",
            "| Key | Value |\n",
            "| --- | --- |\n",
            "| alpha | beta |\n",
            "\n",
            "[id]: https://example.com/reference\n",
            "```\n",
        ],
    ];
    for chunks in streams {
        let (_, render) = assert_rich_stream_matches_full_render(chunks, Some(80));
        assert!(render.has_reference_link_definition);
    }
}

#[test]
fn reference_link_definition_survives_raw_to_rich_render_mode_switch() {
    let cwd = test_cwd();
    let width = Some(80);
    let mut source = String::new();
    let mut render = StreamingRender::new();

    append(
        &mut render,
        &mut source,
        "[id]: https://example.com/reference\n\n",
        width,
        &cwd,
        HistoryRenderMode::Raw,
    );
    render.recompute(
        &source,
        width,
        &cwd,
        HistoryRenderMode::Rich,
        /*inline_visualization_context*/ None,
    );
    for chunk in ["First [reference][id].\n\n", "Later [reference][id].\n"] {
        append_rich_and_assert_matches_full(&mut render, &mut source, chunk, width, &cwd);
    }

    assert!(render.has_reference_link_definition);
}

#[test]
fn incremental_render_does_not_add_blank_line_before_html_block() {
    let streams: &[&[&str]] = &[
        &[
            "Paragraph.\n\n",
            "<div>x</div>\n\n",
            "Following paragraph.\n",
        ],
        &["Paragraph.\n\n<div>x</div>\n\n", "Following paragraph.\n"],
    ];
    for chunks in streams {
        let (_, render) = assert_rich_stream_matches_full_render(chunks, Some(80));
        assert_eq!(
            lines_to_plain_strings(&render.lines),
            vec!["Paragraph.", "<div>x</div>", "", "Following paragraph.",],
        );
    }
}

#[test]
fn incremental_render_preserves_heading_and_normalized_html_seams() {
    let streams: &[&[&str]] = &[
        &["Paragraph.\n\n# Heading\n\n", "Next paragraph.\n"],
        &[
            "Paragraph.\n\n",
            "<div>x</div>\n\n",
            "```markdown\n",
            "| Key | Value |\n",
            "| --- | --- |\n",
            "| alpha | beta |\n",
            "\n",
            "Following paragraph.\n",
            "```\n",
        ],
    ];
    for chunks in streams {
        assert_rich_stream_matches_full_render(chunks, Some(80));
    }
}

#[test]
fn paragraphs_after_unwrapped_table_fence_advance_stable_source() {
    let cwd = test_cwd();
    let width = Some(80);
    let (mut source, mut render) = assert_rich_stream_matches_full_render(
        &[
            "```markdown\n",
            "| Key | Value |\n",
            "| --- | --- |\n",
            "| alpha | beta |\n",
            "```\n\n",
        ],
        width,
    );

    let mut previous_stable_source_len = render.stable_source_len;
    for block in ["<div>Post-fence HTML.</div>\n\n", "First paragraph.\n\n"] {
        append_rich_and_assert_matches_full(&mut render, &mut source, block, width, &cwd);
        assert!(render.stable_source_len > previous_stable_source_len);
        previous_stable_source_len = render.stable_source_len;
    }
}

#[test]
fn shell_pid_preserves_following_equations() {
    for shell in [
        "Shell examples: $HOME and echo $$.",
        "Shell examples: $HOME.",
    ] {
        let source = format!("{shell}\n\nAfter rejected equations: $\\alpha$.\n\n$$\\beta$$");
        for width in [80, 24] {
            let (_, render) = assert_rich_stream_matches_full_render(
                &source.split_inclusive('$').collect::<Vec<_>>(),
                Some(width),
            );
            let expected = format!("{shell}\n\nAfter rejected equations: α.\n\nβ");
            assert_eq!(
                render.lines,
                render_source(
                    &expected,
                    Some(width),
                    &test_cwd(),
                    HistoryRenderMode::Rich,
                    /*inline_visualization_context*/ None,
                )
            );
        }
    }
}

#[test]
fn rejected_math_openers_allow_bounded_incremental_rendering() {
    let cwd = test_cwd();
    for (open, close) in [
        ("echo $$", "$$"),
        ("Equation: \\[", "\\]"),
        (r"\[label\]\*", "\\]"),
    ] {
        let (mut source, mut render) =
            assert_rich_stream_matches_full_render(&[&format!("{open}\n\n")], Some(80));
        let distant_closer = format!("{close}\n\nAfter $\\alpha$.\n\n");
        for chunk in std::iter::repeat_n(
            "An ordinary paragraph that must not retain the entire response.\n\n",
            /*count*/ 160,
        )
        .chain(std::iter::once(distant_closer.as_str()))
        {
            append_rich_and_assert_matches_full(&mut render, &mut source, chunk, Some(80), &cwd);
            assert!(source.len() - render.stable_source_len < 4200);
        }
    }
}

#[test]
fn recompute_with_unused_visualization_context_matches_full_renderer() {
    let cwd = test_cwd();
    let context = InlineVisualizationContext::new(&cwd, ThreadId::new()).unwrap();
    // 引用、代码块、表格和数学公式的样式与超链接元数据必须与原全量路径相同。
    for source in [
        "First paragraph.\n\nSecond paragraph.\n",
        "[link][ref]\n\n[ref]: https://example.com\n",
        "```rust\nlet x = 1;\n```\n",
        "|a|b|\n|-|-|\n|1|2|\n",
        "before $x^2$ after\n",
    ] {
        for width in [None, Some(12), Some(80)] {
            let mut render = StreamingRender::new();
            render.recompute(source, width, &cwd, HistoryRenderMode::Rich, Some(&context));
            assert_eq!(
                render.lines,
                render_source(source, width, &cwd, HistoryRenderMode::Rich, Some(&context))
            );
        }
    }
}

#[test]
fn renderer_stable_line_boundaries_match_independent_prefix_rendering() {
    use crate::markdown_render::ListSpacing;
    let cwd = test_cwd();
    // 前缀独立渲染是旧实现的行边界基准；覆盖列表结束时回写间距、HTML拼接和换行布局。
    let blocks = [
        "paragraph with enough words to wrap across several lines\n\n",
        "- first\n- second long item that wraps\n\n",
        "1. outer\n   - nested\n\n",
        "<div>raw html</div>\n\n",
        "```rust\nlet x = 1;\n```\n\n",
        "> quoted paragraph\n\n",
        "---\n\n",
        "| a | b |\n|---|---|\n| 1 | 2 |\n\n",
    ];
    for first in blocks {
        for last in blocks {
            for width in [None, Some(12), Some(80)] {
                for spacing in [
                    ListSpacing::AfterMultiline,
                    ListSpacing::Compact,
                    ListSpacing::Uniform,
                ] {
                    let source = format!("{first}{last}");
                    let rendered = render_streaming_markdown_agent_with_links_and_cwd(
                        &source,
                        width,
                        Some(&cwd),
                        spacing,
                    );
                    if let Some(boundary) = rendered.last_top_level_block_start {
                        let prefix = render_streaming_markdown_agent_with_links_and_cwd(
                            &source[..boundary],
                            width,
                            Some(&cwd),
                            spacing,
                        );
                        assert_eq!(
                            rendered.last_top_level_rendered_len,
                            Some(prefix.lines.len()),
                            "source: {source:?}, width: {width:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn reference_definitions_reuse_plain_tail_and_fall_back_for_new_reference_syntax() {
    let cwd = test_cwd();
    let mut render = StreamingRender::new();
    let mut source = String::new();
    for chunk in [
        "[known][ref] and [future][later]\n\n[ref]: https://example.com\n\n",
        "plain tail\n\n",
        "more plain words\n\n",
        "[later]: https://example.org\n\n",
        "another tail\n\n",
        "[known][ref]\n\n",
    ] {
        append_rich_and_assert_matches_full(&mut render, &mut source, chunk, Some(24), &cwd);
        if chunk == "more plain words\n\n" {
            assert!(
                render.stable_source_len > 0,
                "reference-free tail should retain its stable prefix"
            );
        }
    }
}

#[test]
fn reference_definition_incomplete_title_keeps_full_render_fallback() {
    let cwd = test_cwd();
    for (open, close) in [('"', '"'), ('\'', '\''), ('(', ')')] {
        let mut render = StreamingRender::new();
        let mut source = String::new();
        for chunk in [
            format!("[known][ref]\n\n[ref]: https://example.com\n{open}unfinished\n"),
            format!("title{close}\n\n"),
            "plain tail\n\n".to_string(),
            "more text\n".to_string(),
        ] {
            append_rich_and_assert_matches_full(&mut render, &mut source, &chunk, Some(24), &cwd);
        }
    }
}

#[test]
fn stable_prefix_reuse_parses_only_the_mutable_source_once() {
    let cwd = test_cwd();
    let mut render = StreamingRender::new();
    let mut source = "first stable paragraph\n\nmutable paragraph\n".to_string();
    crate::markdown_render::take_render_work();
    render.append(
        &source,
        &source,
        Some(24),
        &cwd,
        HistoryRenderMode::Rich,
        None,
    );
    assert_eq!(
        crate::markdown_render::take_render_work(),
        (1, source.len())
    );
    let stable_bytes = render.stable_source_len;
    source.push_str("more text\n\nlast paragraph\n");
    render.append(
        &source,
        "more text\n\nlast paragraph\n",
        Some(24),
        &cwd,
        HistoryRenderMode::Rich,
        None,
    );
    assert_eq!(
        crate::markdown_render::take_render_work(),
        (1, source.len() - stable_bytes)
    );
    assert_eq!(
        render.lines,
        render_source(&source, Some(24), &cwd, HistoryRenderMode::Rich, None)
    );
}

#[test]
fn reference_prefix_blank_line_gate_matches_markdown_ascii_whitespace() {
    let cwd = test_cwd();
    for (separator, reusable) in [
        ("\n\n", true),
        ("\r\n\r\n", true),
        ("\n \n", true),
        ("\n  \n", true),
        // pulldown-cmark 将含 tab 的分隔行末尾换行保留在下一段范围内，
        // 此时边界前没有完整空行，保守路径必须继续全量渲染。
        ("\n\t\n", false),
        ("\n \t\n", false),
        ("\n\u{a0}\n", false),
    ] {
        let mut source =
            format!("[known][ref]\n\n[ref]: https://example.com{separator}plain tail\n");
        let mut render = StreamingRender::new();
        render.recompute(&source, Some(24), &cwd, HistoryRenderMode::Rich, None);
        if reusable {
            assert!(render.stable_source_len > 0, "separator: {separator:?}");
        } else {
            assert_eq!(render.stable_source_len, 0, "separator: {separator:?}");
        }
        assert_eq!(
            render.lines,
            render_source(&source, Some(24), &cwd, HistoryRenderMode::Rich, None)
        );
        // 触发条件和输出等价分别验收；包括保守回退的 tab/NBSP 也必须跨 chunk 等价。
        append_rich_and_assert_matches_full(
            &mut render,
            &mut source,
            "continued tail\n\nnext paragraph\n",
            Some(24),
            &cwd,
        );
    }
}
