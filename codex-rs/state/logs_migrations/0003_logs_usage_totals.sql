-- 容量统计与日志行在同一 SQLite 事务内更新；触发器同时覆盖旧版本进程的写入、
-- 分区裁剪和全局裁剪。只有迁移首次执行扫描，正常批量写入仅查询这一行。
CREATE TABLE logs_usage_totals (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    total_bytes INTEGER NOT NULL,
    row_count INTEGER NOT NULL
);
INSERT INTO logs_usage_totals (singleton, total_bytes, row_count)
SELECT 1, COALESCE(SUM(estimated_bytes), 0), COUNT(*) FROM logs;

CREATE TRIGGER logs_usage_after_insert AFTER INSERT ON logs BEGIN
    UPDATE logs_usage_totals SET
        total_bytes = total_bytes + NEW.estimated_bytes,
        row_count = row_count + 1
    WHERE singleton = 1;
END;
CREATE TRIGGER logs_usage_after_delete AFTER DELETE ON logs BEGIN
    UPDATE logs_usage_totals SET
        total_bytes = total_bytes - OLD.estimated_bytes,
        row_count = row_count - 1
    WHERE singleton = 1;
END;
CREATE TRIGGER logs_usage_after_update AFTER UPDATE OF estimated_bytes ON logs BEGIN
    UPDATE logs_usage_totals SET
        total_bytes = total_bytes + NEW.estimated_bytes - OLD.estimated_bytes
    WHERE singleton = 1;
END;
