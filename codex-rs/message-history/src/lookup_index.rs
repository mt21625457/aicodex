//! 有界、可失效的行偏移索引。只缓存已验证 UTF-8 的行边界，不缓存消息正文。
use std::collections::VecDeque;
use std::fs::Metadata;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

const STRIDE: usize = 256;
const _: () = assert!(STRIDE.is_power_of_two());
const MAX_ANCHORS: usize = 4096;
const MAX_FILES: usize = 16;

#[derive(Clone, PartialEq, Eq)]
struct Revision {
    identity: (u64, u64),
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[cfg(unix)]
fn revision(metadata: &Metadata) -> Option<Revision> {
    use std::os::unix::fs::MetadataExt;
    // 粗粒度时间戳无法可靠识别同长度原地重写，保守退回从头扫描。
    if metadata.mtime_nsec() == 0 || metadata.ctime_nsec() == 0 {
        return None;
    }
    Some(Revision {
        identity: (metadata.dev(), metadata.ino()),
        len: metadata.len(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

#[cfg(not(unix))]
fn revision(_metadata: &Metadata) -> Option<Revision> {
    // Windows 等平台尚无等价的高精度变更标识时不复用索引。
    None
}

struct Entry {
    path: PathBuf,
    revision: Revision,
    anchors: Vec<(usize, u64)>,
    stride: usize,
}

fn cache() -> &'static Mutex<VecDeque<Entry>> {
    static CACHE: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

pub(super) struct ScanCursor {
    path: PathBuf,
    revision: Option<Revision>,
    anchors: Vec<(usize, u64)>,
    stride: usize,
    pub(super) row: usize,
    pub(super) position: u64,
}

impl ScanCursor {
    /// 调用方已持有文件共享锁，metadata 必须从同一已打开句柄在锁内读取。
    pub(super) fn new(path: &Path, metadata: &Metadata, offset: usize, use_index: bool) -> Self {
        // 有容量上限的历史可能原地裁剪；与 batch lookup 一样，不为该模式复用索引。
        let revision = use_index.then(|| revision(metadata)).flatten();
        let (anchors, stride) = revision
            .as_ref()
            .and_then(|revision| {
                let entries = cache().lock().ok()?;
                entries
                    .iter()
                    .find(|entry| entry.path == path && &entry.revision == revision)
                    .map(|entry| (entry.anchors.clone(), entry.stride))
            })
            .unwrap_or_else(|| (Vec::new(), STRIDE));
        let (row, position) = anchors
            .iter()
            .rev()
            .find(|(row, _)| *row <= offset)
            .copied()
            .unwrap_or((0, 0));
        Self {
            path: path.to_owned(),
            revision,
            anchors,
            stride,
            row,
            position,
        }
    }

    pub(super) fn advance(&mut self, bytes: usize) {
        self.row += 1;
        self.position += bytes as u64;
        // 禁用索引的平台或历史模式只线性扫描，不构造最终无法复用的锚点。
        if self.revision.is_some()
            // 初始 stride 为 2 的幂且只翻倍；用掩码避免每个扫描行执行变量除法。
            && self.row & (self.stride - 1) == 0
            && self.anchors.last().is_none_or(|(row, _)| *row < self.row)
        {
            if self.anchors.len() == MAX_ANCHORS {
                // 容量饱和后均匀稀疏化已验证的锚点，并继续覆盖长尾。不能只丢弃
                // 早期锚点，否则反向查询会重新退化为全文扫描。保留的锚点仍来自
                // 同一 revision 的成功 UTF-8 读取，不推算或创建未验证的字节位置。
                let Some(stride) = self.stride.checked_mul(2) else {
                    return;
                };
                self.stride = stride;
                self.anchors.retain(|(row, _)| row.is_multiple_of(stride));
            }
            if self.row & (self.stride - 1) == 0 {
                self.anchors.push((self.row, self.position));
            }
        }
    }

    pub(super) fn save(self) {
        let Some(revision) = self.revision else {
            return;
        };
        let Ok(mut entries) = cache().lock() else {
            return;
        };
        // 文件锁保证读取时内容一致；索引锁只保护小型元数据，不覆盖磁盘 I/O。
        entries.retain(|entry| entry.path != self.path);
        entries.push_front(Entry {
            path: self.path,
            revision,
            anchors: self.anchors,
            stride: self.stride,
        });
        entries.truncate(MAX_FILES);
    }
}

#[cfg(test)]
#[path = "lookup_index_tests.rs"]
mod adaptive_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_index_bounds_scan_and_rejects_changed_files_and_capped_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history");
        std::fs::write(&path, "x\n".repeat(2048)).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let supported = revision(&metadata).is_some();
        let mut cursor = ScanCursor::new(&path, &metadata, 2047, true);
        for _ in 0..2048 {
            cursor.advance(2);
        }
        assert_eq!(cursor.anchors.len(), if supported { 8 } else { 0 });
        cursor.save();
        let cached = ScanCursor::new(&path, &metadata, 2047, true);
        // 该历史尚未触发稀疏化，暖查询最多扫描一个初始 stride。
        assert_eq!(cached.row, if supported { 1792 } else { 0 });
        assert_eq!(cached.position, (cached.row * 2) as u64);
        let mut uncached = ScanCursor::new(&path, &metadata, 2047, false);
        assert_eq!(uncached.row, 0);
        for _ in 0..2048 {
            uncached.advance(2);
        }
        assert_eq!((uncached.row, uncached.position), (2048, 4096));
        assert!(uncached.anchors.is_empty());
        // 长度改变和文件替换都必须放弃旧行边界。
        std::fs::write(&path, "short\n").unwrap();
        assert_eq!(
            ScanCursor::new(&path, &std::fs::metadata(&path).unwrap(), 2047, true).row,
            0
        );
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, "x\n".repeat(2048)).unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert_eq!(
            ScanCursor::new(&path, &std::fs::metadata(&path).unwrap(), 2047, true).row,
            0
        );
    }
}
