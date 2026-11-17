# CORE-005：构建诊断日志重放与 TUI 宏警告

## 根因与责任层

- 排查 sccache 时设置 `SCCACHE_LOG=debug`，客户端日志被 Cargo 保存到
  target 下 `.fingerprint/*/output-*` 文件，后续构建可重放旧时间戳日志。
  本机缓存文件与用户日志中的 `DEBUG sccache::` 行相符。
- TUI 三处 `eyre::bail!` 位于表达式位置，宏内部尾随分号触发
  `semicolon_in_expressions_from_non_local_macros` 未来不兼容警告。
- `Blocking waiting for file lock on package cache` 是 Cargo 等待共享锁，
  不是源码警告；本次不删除锁或终止其他 Cargo 进程。

## 修复

- 默认构建设置 `SCCACHE_LOG=warn`，仍强制使用 sccache。
- 构建输出仅过滤行首带日志前缀、模块为 `sccache::` 的 DEBUG/TRACE 行，
  包括 Cargo 重放内容；不屏蔽 Rust 诊断、sccache WARN/ERROR 或失败退出码。
  `--verbose` 保留原始输出和显式日志设置。无缓存清理、无 daemon 重启。
- `app/reconnect.rs`、`app_backtrack.rs`、`app_server_connection.rs`
  的三处宏调用增加语句分号，不改变错误文本或控制流。
- Code Mode 原子 API 弃用警告见 CORE-003 后续补漏。

## 回归与验收

`python3 -m unittest scripts.test_build scripts.test_build_storage_config scripts.test_build_cache`
通过 47 项。新增合成子进程测试检查日志过滤、诊断保留和失败退出码，
构建命令测试检查默认环境及过滤开关，不包含真实日志或个人路径。

按用户要求未运行 Rust 编译，三处 Rust 警告消除及完整构建待用户验收。
提交可通过本文件的 Git 历史查询；未进行包内或安装后验收。
