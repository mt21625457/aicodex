use super::MAX_FRAGMENT_BYTES;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

const FRAGMENT_PLACEHOLDER: &str = "<!--__INLINE_VISUALIZATION_FRAGMENT__-->";
const VIEWER_DIRECTORY_NAME: &str = ".codex-viewers";

// Keep these assets in sync with the bundled visualize skill's browser renderer.
const VIEWER_STYLESHEET: &str = include_str!("../../assets/inline_visualization/visualize.css");
const VIEWER_RUNTIME: &str = include_str!("../../assets/inline_visualization/visualize.html");

const FRAME_CSP: &str = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval' blob: data: https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://esm.sh https://fonts.bunny.net https://fonts.googleapis.com https://fonts.gstatic.com https://unpkg.com; style-src 'unsafe-inline' blob: data: https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://esm.sh https://fonts.bunny.net https://fonts.googleapis.com https://fonts.gstatic.com https://unpkg.com; img-src blob: data: https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://esm.sh https://fonts.bunny.net https://fonts.googleapis.com https://fonts.gstatic.com https://unpkg.com; font-src blob: data: https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://esm.sh https://fonts.bunny.net https://fonts.googleapis.com https://fonts.gstatic.com https://unpkg.com; media-src blob: data:; worker-src blob:; connect-src blob: data:; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";
const SHELL_STYLE: &str = ":root{color-scheme:light dark;background:light-dark(rgb(255 255 255), rgb(24 24 24))}html,body{margin:0}body{box-sizing:border-box;padding:1rem;background:inherit}iframe{display:block;width:100%;max-width:736px;height:calc(100vh - 2rem);margin:0 auto;border:0}";

/// 缓存生成文档的完整输入，而不是生成后的大文档；仅在成功持久化后更新。
/// 标题参与文档生成，必须与片段一起比较，不能只依赖文件长度或时间戳。
#[derive(Debug)]
pub(super) struct ViewerSource {
    fragment: String,
    title: String,
}

impl ViewerSource {
    fn matches(&self, fragment: &str, title: &str) -> bool {
        self.fragment == fragment && self.title == title
    }
}

#[cfg(test)]
thread_local! {
    static RENDER_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn materialize_document(
    path: &Path,
    viewer_dir: &Path,
    materialized_viewers: &Mutex<HashMap<PathBuf, ViewerSource>>,
) -> std::io::Result<PathBuf> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FRAGMENT_BYTES {
        return Err(std::io::Error::other("invalid visualization fragment"));
    }

    let fragment = fs::read_to_string(path)?;
    let title = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Visualization")
        .replace('-', " ");

    let viewer_dir = viewer_dir.join(VIEWER_DIRECTORY_NAME);
    fs::create_dir_all(&viewer_dir)?;
    if fs::canonicalize(&viewer_dir)? != viewer_dir {
        return Err(std::io::Error::other(
            "visualization viewer directory must not contain symbolic links",
        ));
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("visualization fragment has no file name"))?;
    let viewer_path = viewer_dir.join(file_name);
    // 命中仍需执行上面的读取及路径安全检查，不能掩盖源文件更新或目录错误。
    // 命中时不检查外部是否删改了生成文件，保持原缓存既有行为。
    {
        let cached = materialized_viewers
            .lock()
            .map_err(|_| std::io::Error::other("visualization viewer cache is unavailable"))?;
        if cached
            .get(&viewer_path)
            .is_some_and(|source| source.matches(&fragment, &title))
        {
            return Ok(viewer_path);
        }
    }
    // 大文档构造保持在全局锁外；再次加锁后复核并发请求是否已经生成同一内容。
    let document = render_fragment(&fragment, &title);
    let mut materialized_viewers = materialized_viewers
        .lock()
        .map_err(|_| std::io::Error::other("visualization viewer cache is unavailable"))?;
    if materialized_viewers
        .get(&viewer_path)
        .is_some_and(|source| source.matches(&fragment, &title))
    {
        return Ok(viewer_path);
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&viewer_dir)?;
    temporary.write_all(document.as_bytes())?;
    temporary.flush()?;
    temporary
        .persist(&viewer_path)
        .map_err(|error| error.error)?;
    materialized_viewers.insert(viewer_path.clone(), ViewerSource { fragment, title });
    Ok(viewer_path)
}

fn render_fragment(fragment: &str, title: &str) -> String {
    #[cfg(test)]
    RENDER_COUNT.with(|count| count.set(count.get() + 1));
    let runtime = VIEWER_RUNTIME.replacen(FRAGMENT_PLACEHOLDER, fragment, /*count*/ 1);
    let escaped_title = escape_html(title);
    let frame = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"referrer\" content=\"no-referrer\"><meta http-equiv=\"Content-Security-Policy\" content=\"{FRAME_CSP}\"><title>{escaped_title}</title><style>{VIEWER_STYLESHEET}\nhtml>body{{padding:0}}</style></head><body>{runtime}</body></html>"
    );

    // A srcdoc frame inherits its parent's CSP, so the shell must grant every
    // resource type that the stricter frame CSP may use. The frame itself stays
    // sandboxed without allow-same-origin.
    let shell_csp = FRAME_CSP.replace("frame-src 'none'", "frame-src 'self'");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"referrer\" content=\"no-referrer\"><meta http-equiv=\"Content-Security-Policy\" content=\"{shell_csp}\"><title>{escaped_title}</title><style>{SHELL_STYLE}</style></head><body><iframe sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\" title=\"{escaped_title}\" srcdoc=\"{}\"></iframe></body></html>",
        escape_html(&frame)
    )
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
#[path = "viewer_tests.rs"]
mod tests;
