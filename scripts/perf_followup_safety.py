#!/usr/bin/env python3
"""验证性能后续方案的适用条件；不修改生产数据库、历史文件或生产算法。

SQLite 使用仓库真实 migrations 与生产裁剪 SQL；历史部分是设计门控原型，
不能冒充 Rust 文件锁、故障注入或 Windows 的集成验收。
"""

from __future__ import annotations

import argparse
import itertools
import json
from pathlib import Path
import re
import sqlite3
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
I64_MAX = 2**63 - 1


def load_prune_sql() -> str:
    source = (ROOT / "codex-rs/state/src/runtime/logs.rs").read_text()
    match = re.search(r'async fn prune_global_logs[\s\S]*?r#"([\s\S]*?)"#', source)
    if match is None:
        raise RuntimeError("生产裁剪 SQL 结构已变化，需人工重新核对 oracle")
    return match.group(1)


def make_database() -> sqlite3.Connection:
    connection = sqlite3.connect(":memory:")
    for migration in sorted((ROOT / "codex-rs/state/logs_migrations").glob("*.sql")):
        connection.executescript(migration.read_text())
    return connection


def fill_rows(connection: sqlite3.Connection, newest_weights: tuple[int, ...]) -> None:
    connection.execute("DELETE FROM logs")
    # 从最旧插入，避免极端反例在 INSERT 触发器中先溢出；被验证的是窗口 SUM。
    connection.executemany(
        "INSERT INTO logs(ts,ts_nanos,level,target,estimated_bytes) VALUES(?,0,'INFO','probe',?)",
        enumerate(reversed(newest_weights)),
    )


def read_rows(connection: sqlite3.Connection) -> list[tuple[int, int]]:
    return connection.execute(
        "SELECT id,estimated_bytes FROM logs ORDER BY ts DESC,ts_nanos DESC,id DESC"
    ).fetchall()


def boundary_prototype(rows: list[tuple[int, int]], byte_cap: int, row_cap: int) -> list[tuple[int, int]]:
    # 这只是被检验的候选算法，故意不加安全域回退，以展示错误输入的反例。
    retained = list(rows)
    total = sum(weight for _, weight in retained)
    while retained and (total > byte_cap or len(retained) > row_cap):
        _, removed = retained.pop()
        total -= removed
    return retained


def run_oracle(connection: sqlite3.Connection, sql: str, byte_cap: int, row_cap: int) -> list[tuple[int, int]]:
    # 保留生产函数现有的全局汇总预检查，不总是无条件执行 DELETE。
    total, count = connection.execute(
        "SELECT total_bytes,row_count FROM logs_usage_totals WHERE singleton=1"
    ).fetchone()
    if total > byte_cap or count > row_cap:
        connection.execute(sql, (byte_cap, row_cap))
    return read_rows(connection)


def check_sqlite(max_rows: int) -> dict:
    sql = load_prune_sql()
    connection = make_database()
    verified = 0
    for length in range(max_rows + 1):
        for weights in itertools.product(range(5), repeat=length):
            fill_rows(connection, weights)
            original = read_rows(connection)
            for byte_cap in range(13):
                for row_cap in range(max_rows + 2):
                    connection.execute("SAVEPOINT candidate")
                    actual = run_oracle(connection, sql, byte_cap, row_cap)
                    expected = boundary_prototype(original, byte_cap, row_cap)
                    assert actual == expected, (weights, byte_cap, row_cap, actual, expected)
                    totals = connection.execute(
                        "SELECT total_bytes,row_count FROM logs_usage_totals WHERE singleton=1"
                    ).fetchone()
                    assert totals == (sum(weight for _, weight in actual), len(actual))
                    connection.execute("ROLLBACK TO candidate")
                    connection.execute("RELEASE candidate")
                    verified += 1

    fill_rows(connection, (6, -4, 6))
    original = read_rows(connection)
    candidate = boundary_prototype(original, 5, 100)
    actual = run_oracle(connection, sql, 5, 100)
    assert [weight for _, weight in actual] == [-4]
    assert [weight for _, weight in candidate] == [6, -4]

    fill_rows(connection, (I64_MAX, 1, -I64_MAX))
    original = read_rows(connection)
    assert connection.execute("SELECT total_bytes,row_count FROM logs_usage_totals").fetchone() == (1, 3)
    connection.execute("SAVEPOINT overflow_case")
    try:
        run_oracle(connection, sql, I64_MAX, 2)
    except sqlite3.OperationalError as error:
        assert "integer overflow" in str(error), str(error)
        overflow_error = str(error)
    else:
        raise AssertionError("原窗口算法应触发 integer overflow")
    connection.execute("ROLLBACK TO overflow_case")
    connection.execute("RELEASE overflow_case")
    assert read_rows(connection) == original
    assert [weight for _, weight in boundary_prototype(original, I64_MAX, 2)] == [I64_MAX]

    # SQLite INTEGER affinity 并非严格类型约束，不能把 schema 声明当作安全域证明。
    connection.execute("DELETE FROM logs")
    connection.execute(
        "INSERT INTO logs(ts,ts_nanos,level,target,estimated_bytes) VALUES(0,0,'INFO','probe',0.5)"
    )
    assert connection.execute("SELECT typeof(estimated_bytes) FROM logs").fetchone() == ("real",)
    connection.close()
    return {
        "sqlite_version": sqlite3.sqlite_version,
        "nonnegative_small_domain_cases": verified,
        "negative_counterexample": {"oracle": [-4], "unsafe_boundary": [6, -4]},
        "overflow_counterexample": {"oracle_error": overflow_error, "unsafe_boundary": [I64_MAX]},
        "non_integer_storage_allowed": True,
        "scope": "真实 SQL 及触发器；不代表 SQLx 错误解码、并发事务或性能验收",
    }


def can_promote_revision(**conditions: bool) -> bool:
    # 设计门控原型：所有事实必须在既有独占句柄/成功写入路径中成立。
    return all(conditions.values())


def check_history() -> dict:
    # 第 256 行没有换行。旧扫描可走到 EOF，但追加会延长该行，不产生第 257 行。
    original = b"row\n" * 255 + "尾行".encode()
    appended = b"suffix\n"
    with tempfile.TemporaryDirectory(prefix="aicodex-history-proof-") as directory:
        path = Path(directory) / "history"
        path.write_bytes(original)
        old_eof = path.stat().st_size
        with path.open("ab") as writer:
            writer.write(appended)
        full = path.read_bytes().splitlines()
        with path.open("rb") as reader:
            reader.seek(old_eof)
            unsafe_reused_line = reader.readline()
        assert len(full) == 256
        assert full[-1] == "尾行".encode() + b"suffix"
        assert unsafe_reused_line == b"suffix\n"
        assert not original.endswith(b"\n")

    safe = dict(
        supported_revision=True,
        original_revision_matches=True,
        same_open_handle_and_exclusive_lock=True,
        no_configured_limit=True,
        original_empty_or_newline_terminated=True,
        write_all_succeeded=True,
        flush_succeeded=True,
        limit_step_succeeded_without_truncation=True,
        observed_identity_and_length_match=True,
        cache_revision_still_matches=True,
        optional_probes_succeeded=True,
    )
    assert can_promote_revision(**safe)
    for rejected in safe:
        assert not can_promote_revision(**(safe | {rejected: False})), rejected
    return {
        "unterminated_tail_eof_anchor_counterexample": True,
        "promotion_gate_cases": 1 + len(safe),
        "scope": "真实临时文件 EOF 反例及设计门控模型；未实现生产跨追加复用，未验证文件锁或 Windows",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--max-rows", type=int, choices=range(1, 5), default=3)
    parser.add_argument("--markdown-rlib", type=Path, help="已有 pulldown_cmark rlib；可选，不触发 Cargo")
    parser.add_argument("--output", type=Path, help="将结果另存 JSON")
    args = parser.parse_args()
    result = {"sqlite": check_sqlite(args.max_rows), "history": check_history()}
    if args.markdown_rlib:
        library = args.markdown_rlib.resolve()
        with tempfile.TemporaryDirectory(prefix="aicodex-markdown-proof-") as directory:
            binary = Path(directory) / "probe"
            subprocess.run([
                "rustc", "--edition=2024", "-O", str(ROOT / "scripts/perf_followup_markdown_probe.rs"),
                "-L", f"dependency={library.parent}", "--extern", f"pulldown_cmark={library}",
                "-o", str(binary),
            ], check=True)
            output = subprocess.check_output([str(binary)], text=True).strip()
            result["markdown_parser"] = output
    serialized = json.dumps(result, ensure_ascii=False, indent=2)
    if args.output:
        args.output.write_text(serialized + "\n")
    print(serialized)


if __name__ == "__main__":
    main()
