use super::*;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;

#[test]
fn adaptive_anchors_cover_prefix_and_tail_with_bounded_storage() {
    // 直接测试平台无关的锚点策略；生产中的非 Unix revision 仍返回 None。
    let mut cursor = ScanCursor {
        path: PathBuf::new(),
        revision: Some(Revision {
            identity: (1, 1),
            len: 0,
            modified: (1, 1),
            changed: (1, 1),
        }),
        anchors: Vec::new(),
        stride: STRIDE,
        row: 0,
        position: 0,
    };
    let total = MAX_ANCHORS * STRIDE * 4 + STRIDE * 4 + 129;
    for row in 0..total {
        // 变长行使 byte offset 不能由行号乘固定长度推算。
        cursor.advance(row % 7 + 1);
        assert!(cursor.anchors.len() <= MAX_ANCHORS);
    }
    assert_eq!(cursor.stride, STRIDE * 8);
    assert_eq!(cursor.anchors.first().unwrap().0, cursor.stride);
    let expected_position = |row: usize| {
        let remainder = row % 7;
        ((row / 7) * 28 + remainder * (remainder + 1) / 2) as u64
    };
    for &(row, position) in &cursor.anchors {
        assert_eq!(row % cursor.stride, 0);
        assert_eq!(position, expected_position(row));
    }
    for offset in [0, 17, total - 1, total / 2, STRIDE * 3, total - 77] {
        let start = cursor
            .anchors
            .iter()
            .rev()
            .find(|(row, _)| *row <= offset)
            .map_or(0, |(row, _)| *row);
        assert!(offset - start < cursor.stride);
    }
}

fn read_indexed(path: &Path, offset: usize) -> std::io::Result<(String, usize, usize)> {
    let file = File::open(path)?;
    file.lock_shared()?;
    let mut cursor = ScanCursor::new(path, &file.metadata()?, offset, true);
    let mut reader = BufReader::new(&file);
    reader.seek(SeekFrom::Start(cursor.position))?;
    let mut line = String::new();
    let mut scanned = 0;
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            let stride = cursor.stride;
            cursor.save();
            return Ok((line, scanned, stride));
        }
        scanned += 1;
        let found = cursor.row == offset;
        cursor.advance(bytes);
        if found {
            let stride = cursor.stride;
            cursor.save();
            return Ok((line, scanned, stride));
        }
    }
}

#[test]
fn real_file_random_and_reverse_lookups_keep_content_and_bound_scans() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history");
    let rows = MAX_ANCHORS * STRIDE + 1025;
    {
        let mut writer = BufWriter::new(File::create(&path).unwrap());
        for row in 0..rows {
            writeln!(writer, "消息 {row}\r").unwrap();
        }
        writer.flush().unwrap();
    }
    let supported = revision(&std::fs::metadata(&path).unwrap()).is_some();
    let (line, scanned, stride) = read_indexed(&path, rows - 1).unwrap();
    assert_eq!(line, format!("消息 {}\r\n", rows - 1));
    assert_eq!(scanned, rows);
    assert_eq!(stride, if supported { STRIDE * 2 } else { STRIDE });
    // 覆盖前缀、稀疏化边界、长尾、反向与非顺序查询，断言真实读取行数而非耗时。
    for offset in [rows - 1, 0, rows / 2, rows - 2, STRIDE - 1, 712_379, 31] {
        let (line, scanned, stride) = read_indexed(&path, offset).unwrap();
        assert_eq!(line, format!("消息 {offset}\r\n"));
        if supported {
            assert!(scanned <= stride, "offset={offset}, scanned={scanned}");
        } else {
            assert_eq!(scanned, offset + 1);
        }
    }
    // 追加仍使整个旧 revision 失效；本次不延长旧锚点的适用代际。
    // Windows 不能锁住仅 append 模式的句柄；与生产追加路径一样保留读取权限。
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .open(&path)
        .unwrap();
    file.lock().unwrap();
    writeln!(file, "消息 {rows}\r").unwrap();
    file.flush().unwrap();
    assert_eq!(
        ScanCursor::new(&path, &file.metadata().unwrap(), rows, true).row,
        0
    );
    drop(file);
    let (line, scanned, _) = read_indexed(&path, rows).unwrap();
    assert_eq!(line, format!("消息 {rows}\r\n"));
    assert_eq!(scanned, rows + 1);
}

#[test]
fn rewritten_invalid_utf8_prefix_cannot_be_skipped_by_cached_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history");
    let contents = "x\n".repeat(2048);
    std::fs::write(&path, &contents).unwrap();
    assert_eq!(read_indexed(&path, 2047).unwrap().0, "x\n");
    // 同长度原地重写并明确推进 mtime，避免测试依赖文件系统时钟分辨率。
    let mut file = OpenOptions::new().write(true).open(&path).unwrap();
    file.lock().unwrap();
    file.write_all(&[0xff]).unwrap();
    file.flush().unwrap();
    let modified = file.metadata().unwrap().modified().unwrap() + std::time::Duration::from_secs(1);
    file.set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(
        ScanCursor::new(&path, &file.metadata().unwrap(), 2047, true).row,
        0
    );
    drop(file);
    assert_eq!(
        read_indexed(&path, 2047).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    // 截断与替换同样不继承旧锚点。
    std::fs::write(&path, "new\n").unwrap();
    assert_eq!(read_indexed(&path, 0).unwrap().0, "new\n");
    let replacement = dir.path().join("replacement");
    std::fs::write(&replacement, "replacement\n").unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    assert_eq!(read_indexed(&path, 0).unwrap().0, "replacement\n");
}
