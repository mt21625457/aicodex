//! 依赖仓库已有 pulldown-cmark 产物验证引用的非局部影响；不是新的增量 renderer。
use pulldown_cmark::{Event, Options, Parser, Tag};

fn links(source: &str) -> Vec<(String, String)> {
    Parser::new_ext(source, Options::empty())
        .filter_map(|event| match event {
            Event::Start(Tag::Link {
                dest_url, title, ..
            }) => Some((dest_url.into_string(), title.into_string())),
            _ => None,
        })
        .collect()
}

fn main() {
    // 未解析引用也必须记录依赖；后方定义会改变已经显示的早期块。
    let prefix = "Earlier [reference][id].\n\n";
    assert!(links(prefix).is_empty());
    let complete = format!("{prefix}[id]: /first\n");
    assert_eq!(links(&complete), vec![("/first".into(), String::new())]);
    // 重复定义 first-wins：后来追加的定义不应让早期块切换目的地。
    assert_eq!(
        links(&format!("{complete}\n[id]: /second\n")),
        links(&complete)
    );
    assert_eq!(
        links("[link][ A  B ]\n\n[a b]: /normalized\n"),
        vec![("/normalized".into(), String::new())]
    );
    assert!(links("[link][id]\n\n```\n[id]: /not-a-definition\n```\n").is_empty());
    assert_eq!(
        links("[link][id]\n\n[id]: /target\n  \"multi\nline title\"\n"),
        vec![("/target".into(), "multi\nline title".into())]
    );
    // 在每个字符边界重新解析，确认链接直到定义完成才出现，不能提前固化前缀。
    let mut seen_unresolved = false;
    let mut seen_resolved = false;
    for boundary in complete
        .char_indices()
        .map(|(index, _)| index)
        .chain([complete.len()])
    {
        if boundary >= prefix.len() {
            let resolved = !links(&complete[..boundary]).is_empty();
            seen_unresolved |= !resolved;
            seen_resolved |= resolved;
        }
    }
    assert!(seen_unresolved && seen_resolved);
    println!("6 parser dependency checks passed; no incremental TUI equivalence claim");
}
