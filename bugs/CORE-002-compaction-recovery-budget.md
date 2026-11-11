# CORE-002：压缩恢复预算与诊断

- 状态：本地源码修复及聚焦验证完成；未打包、未安装，真实链路验收待完成。
- 修复提交：本记录与实现同批本地提交，可通过 `git log -- bugs/CORE-002-compaction-recovery-budget.md` 定位。
- 关联：应用OpenSpec fix-compaction-and-team-recovery。

## 已证实机制

输入估算242313超过242000阈值；窗口370000减输出预留128000决定阈值，UI有效窗口351500并非压缩阈值。两次连接重建后仍生成，到300002ms总预算截止，最近进展77ms。服务器观察下游关闭、上游仍运行；前两次断连责任未知，不能归责上游或代理。

## 修复合同

保留120秒idle、采用300秒软截止及600秒全局硬截止。只有当前有效生成的attempt可进入宽限，重试必须在剩余软预算内预留退避及120秒idle。重试不重置全局时钟。600秒是初始有界恢复参数，不保证网络不再断连。

watchdog记录安全类别及attempt，错误上下文区分idle/total/retry_budget。超时、取消、incomplete不提交部分摘要，保留用户、助手、工具调用和结果。网络计时器不包围成功后的历史提交事务。输入阈值计算未修改，日志明确输出预留。

## 验证

生产watchdog直接include的隔离harness中，active_summary_survives_soft_deadline旧实现301秒失败，修改后通过。完整内核 cargo test -p codex-core --lib compact：147通过、0失败、1已有忽略。新增软截止、硬截止、metadata、空输出项与无delta完整内容、两次重试、退避准入和timeout/cancel/incomplete历史保留覆盖。原target/debug写权限受限，使用临时target完成验证。

源码通过不代表包内运行；仍需合成大历史真实网关故障注入、实际代理链路及安装后继续发送验收。

## 责任与规模

compaction_budget.rs拥有独立策略，compaction_progress.rs只检查完整输出项是否有内容；compaction_watchdog.rs拥有等待/诊断；compact.rs仅接入attempt和重试准入。compact.rs 1062→1067行，session/turn.rs 3987→3990行仅修正阈值诊断。无新增预算职责堆入既有大文件。

## 2026-11-11 审核修复

发现诊断分类缺陷：executor 延迟调度时，硬截止和 idle 截止可能同时已过期，按当前 idle 时长判断会把先发生的硬截止误报为 idle。责任层为 `compaction_watchdog.rs`，现按最先到期的 deadline 分类，不改变预算或历史提交行为。

新增 `delayed_poll_preserves_the_first_expired_timeout_kind`：合成输出持续至550秒，硬截止600秒、idle截止670秒，700秒恢复调度仍须报告 `total`。团队配置证据另补实际 `snapshot` 测试，验证来自已加载配置、不读取角色文件，非 UTF-8 路径不生成误导性证据。

本次验证（从 `codex-rs` 执行，设置 `RUSTC_WRAPPER=`、`RUST_MIN_STACK=16777216`）：

- `cargo test -p codex-core --lib compact`：沙箱内134通过、14项本地端口绑定被拒、1忽略；获准沙箱外重跑148通过、0失败、1既有忽略。
- `cargo test -p codex-core --lib wait_`：52通过；`cargo test -p codex-core --lib client_`：40通过。
- `cargo test -p codex-protocol -p codex-app-server-protocol --lib`：通过，覆盖错误序列化、新旧字段及版本化 schema 一致性。
- `cargo test -p codex-api --lib`：沙箱内250通过、21项本地端口绑定被拒；获准沙箱外重跑271通过。
- `cargo test -p codex-guardian-reviewer --lib retry`：12通过。
- `cargo test -p codex-app-server --lib thread_agent_team_policy`：2通过。
- Python SDK 的 `test_contract_generation.py`、`test_public_api_runtime_behavior.py`、`test_public_api_signatures.py`：59通过，包含重新生成产物一致性。
- 全仓 `cargo fmt --all -- --check` 仍报告未改动的 `multi_agents_spec_tests.rs` 既有格式差异；本次新增测试文件的直接 rustfmt 检查通过。

以上属于源码及本机模拟验证，不构成真实服务、包内或安装后验收。

## 0.167.1 合并兼容：本地估算与服务端用量边界

上游新增“最后服务端用量覆盖的历史位置”，与本地全历史 `context_tokens` 估算合并后，如果仍从服务端边界追加尾部，会重复计入估算已包含的无 usage 输出和工具结果；无 usage 时调用 `set_context_usage` 还会覆盖最后一条服务端响应用量。

责任层为 `context_manager/history.rs` 和 `session/turn.rs`。本地全上下文估算现在具有独立覆盖边界，保留服务端响应用量及其边界；估算与安装边界在同一 state 锁内完成，避免把并发新增输入错误标为已覆盖。服务端新用量和历史重写使本地边界失效。

新增 `local_context_estimate_has_independent_usage_coverage` 回归：服务端 usage → 无 usage 输出 → 本地估算 → 新工具结果，分别检查上下文视图不重复、无 context 视图不遗漏、spending-only 保留边界、新服务端 usage 替换本地视图。聚焦命令：`cargo test -p codex-core --lib local_context_estimate_has_independent_usage_coverage`。本次合并尚未提交；测试运行结果由合并验收记录补充，不能将静态检查视为运行通过。

交叉审核追加 `history_rewrite_invalidates_only_covered_local_context_estimate`：替换历史或删除首项时，同时清除失效本地覆盖边界和对应 context 字段，保留服务端响应 usage；不存在本地覆盖边界的 provider context 不受影响。否则仅清边界会让旧的全历史估算重新参与计算。

合并后源码回归：nextest 跨模块聚焦集 1160 项全部通过，其中包含上述两项新增测试、`context_manager::history`、`compact`、`wait_`、`client_`、工具规划及模型目录回归。使用仓库校验的 V8 产物、禁用本机不可用的 sccache，并获准在沙箱外绑定测试模拟服务端口；未执行真实服务、包内或安装验收。

扩展串行矩阵（关闭重试）运行 1613 项：1612 通过，1 项 Guardian initialize 超时；本问题的 history、compact 及场景回归通过。整组仍有失败，不能称为全量验收通过；模拟测试不代表真实代理链路或安装验收。

后续复用相同构件，扩大矩阵至 1729 项（覆盖此前全部 1613 项），串行且关闭重试，全部通过；最终源码全工作区 `check --offline --tests` 通过。Guardian 首次启动差异及原始失败边界见 CORE-001 合并回归记录；没有通过修改业务断言或扩大 timeout 取得通过。合并未提交，未执行真实代理链路、包内或安装验收。
