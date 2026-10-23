use super::*;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let source = root.join("sample-title.html");
    fs::write(&source, "<div>first</div>").unwrap();
    let viewers = root.join("viewers");
    (temp, source, viewers)
}

#[test]
fn runtime_has_one_fragment_placeholder() {
    // 输入相同当且仅当生成文档相同依赖模板确实嵌入完整片段，防止模板变更破坏缓存依据。
    assert_eq!(VIEWER_RUNTIME.matches(FRAGMENT_PLACEHOLDER).count(), 1);
}

#[test]
fn warm_hit_skips_render_and_preserves_document() {
    let (_temp, source, viewers) = fixture();
    let cache = Mutex::new(HashMap::new());
    RENDER_COUNT.with(|count| count.set(0));
    let path = materialize_document(&source, &viewers, &cache).unwrap();
    let contents = fs::read(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(
        contents,
        render_fragment("<div>first</div>", "sample title").as_bytes()
    );
    RENDER_COUNT.with(|count| count.set(0));
    assert_eq!(
        materialize_document(&source, &viewers, &cache).unwrap(),
        path
    );
    assert_eq!(RENDER_COUNT.with(std::cell::Cell::get), 0);
    assert_eq!(fs::read(&path).unwrap(), contents);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);

    // 缓存命中原本不修复外部删除的生成文件，此优化只省去文档构造。
    fs::remove_file(&path).unwrap();
    assert_eq!(
        materialize_document(&source, &viewers, &cache).unwrap(),
        path
    );
    assert!(!path.exists());
    assert_eq!(RENDER_COUNT.with(std::cell::Cell::get), 0);
}

#[test]
fn same_length_change_rebuilds_from_current_fragment() {
    let (_temp, source, viewers) = fixture();
    let cache = Mutex::new(HashMap::new());
    let path = materialize_document(&source, &viewers, &cache).unwrap();
    let replacement = "<div>other</div>";
    fs::write(&source, replacement).unwrap();
    RENDER_COUNT.with(|count| count.set(0));
    assert_eq!(
        materialize_document(&source, &viewers, &cache).unwrap(),
        path
    );
    assert_eq!(RENDER_COUNT.with(std::cell::Cell::get), 1);
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        render_fragment(replacement, "sample title")
    );
}

#[test]
fn warm_cache_preserves_source_errors_before_rendering() {
    let (_temp, source, viewers) = fixture();
    let cache = Mutex::new(HashMap::new());
    materialize_document(&source, &viewers, &cache).unwrap();
    RENDER_COUNT.with(|count| count.set(0));
    fs::write(&source, [0xff]).unwrap();
    assert_eq!(
        materialize_document(&source, &viewers, &cache)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    fs::File::create(&source)
        .unwrap()
        .set_len(MAX_FRAGMENT_BYTES + 1)
        .unwrap();
    assert_eq!(
        materialize_document(&source, &viewers, &cache)
            .unwrap_err()
            .to_string(),
        "invalid visualization fragment"
    );
    fs::remove_file(&source).unwrap();
    assert_eq!(
        materialize_document(&source, &viewers, &cache)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    assert_eq!(RENDER_COUNT.with(std::cell::Cell::get), 0);
}

#[test]
fn failed_persist_does_not_publish_new_source() {
    let (_temp, source, viewers) = fixture();
    let cache = Mutex::new(HashMap::new());
    let path = materialize_document(&source, &viewers, &cache).unwrap();
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    fs::write(&source, "<div>other</div>").unwrap();
    assert!(materialize_document(&source, &viewers, &cache).is_err());
    assert!(
        cache
            .lock()
            .unwrap()
            .get(&path)
            .unwrap()
            .matches("<div>first</div>", "sample title")
    );
    fs::remove_dir(&path).unwrap();
    materialize_document(&source, &viewers, &cache).unwrap();
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        render_fragment("<div>other</div>", "sample title")
    );
}

#[test]
fn concurrent_same_and_distinct_paths_keep_complete_documents() {
    let (_temp, source, viewers) = fixture();
    let second = source.with_file_name("other-title.html");
    fs::write(&second, "<div>second</div>").unwrap();
    let cache = Mutex::new(HashMap::new());
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..8)
            .map(|index| {
                let input = if index % 2 == 0 { &source } else { &second };
                let cache = &cache;
                let viewers = &viewers;
                scope.spawn(move || materialize_document(input, viewers, cache).unwrap())
            })
            .collect();
        for job in jobs {
            job.join().unwrap();
        }
    });
    for (input, fragment, title) in [
        (&source, "<div>first</div>", "sample title"),
        (&second, "<div>second</div>", "other title"),
    ] {
        RENDER_COUNT.with(|count| count.set(0));
        let path = materialize_document(input, &viewers, &cache).unwrap();
        assert_eq!(RENDER_COUNT.with(std::cell::Cell::get), 0);
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            render_fragment(fragment, title)
        );
    }
    assert_eq!(cache.lock().unwrap().len(), 2);
}

#[cfg(unix)]
#[test]
fn warm_hit_still_rejects_symlinked_viewer_directory() {
    let (_temp, source, viewers) = fixture();
    let cache = Mutex::new(HashMap::new());
    materialize_document(&source, &viewers, &cache).unwrap();
    let original = viewers.join(VIEWER_DIRECTORY_NAME);
    let renamed = viewers.join("original");
    fs::rename(&original, &renamed).unwrap();
    std::os::unix::fs::symlink(&renamed, &original).unwrap();
    RENDER_COUNT.with(|count| count.set(0));
    assert_eq!(
        materialize_document(&source, &viewers, &cache)
            .unwrap_err()
            .to_string(),
        "visualization viewer directory must not contain symbolic links"
    );
    assert_eq!(RENDER_COUNT.with(std::cell::Cell::get), 0);
}
