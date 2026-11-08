# CORE-002：原子更新 API 弃用警告

## 根因与修复边界

Rust 1.99 将原子类型的 `fetch_update` 标记为弃用；替代名称
`try_update` 自 Rust 1.95 稳定，与仓库声明的工具链版本兼容。
旧名称是新名称的别名，本次替换以下两处方法名，并说明容量拒绝的返回语义，不改变内存顺序、
闭包、返回值处理或并发行为：

- `codex-rs/diagnostics/src/lib.rs`：诊断计数递减，零值不下溢。
- `codex-rs/core/src/tools/executed_tool_calls.rs`：直接调用计数预留，达到上限时拒绝预留。

## 回归入口与验收边界

- 在 `codex-rs/` 执行 `cargo test -p codex-diagnostics --lib --offline`：
  4 项通过，覆盖零值递减、竞争更新、guard 生命周期及快照注册。
- 在 `codex-rs/` 使用 Rust 1.95 执行
  `cargo test -p codex-core --lib direct_pending_limit_releases_on_completion_or_dropped_future --offline`：
  1 项通过，覆盖容量上限、完成释放、取消释放及失效预留释放。
  本机编译缓存包装器曾在依赖编译阶段异常退出；通过进程级
  `RUSTC_WRAPPER=''` 直接使用编译器完成验证，不改变仓库配置。
- 在应用 `rust/` 使用 Rust 1.99.1 执行 `cargo check --offline` 已通过，
  两个依赖的编译输出不再出现上述警告；旧源码会产生这两条警告。
- 应用 `npm run typecheck` 通过。`npm run guard` 在预检查阶段被
  `src/features/dynamicTools/validators.generated.js` 过期阻断；
  生成脚本、输入和产物均与 HEAD 相同，本次不修改动态工具生成文件。
- 本次两个生产文件分别保持 211 行、758 行，无新增职责。
- 未进行打包、安装后验收或真实服务验证。修复提交可通过
  `git log --oneline -- bugs/CORE-002-atomic-update-deprecation.md` 查询。
