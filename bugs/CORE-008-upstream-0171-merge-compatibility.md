# CORE-008 合并 0.171.0 后的兼容回归

- 合并双方为本地 `5449d525d2` 与上游同步 `b4fa07364e`。初始冲突涉及 134 个路径。
- 修复及验收记录随合并保存，可用 `git log -- bugs/CORE-008-upstream-0171-merge-compatibility.md` 定位。

## 根因与修复

- 引擎和 app-server 的接口同时演进。补齐自动合并漏接的已解析回合输入、请求预算、响应结束信息、MCP 服务键和线程列表过滤参数，保留本地的恢复、会话和错误合同。
- 模型目录同时出现 `supports_reasoning_summary_parameter` 及其反序列化别名 `supports_reasoning_summaries`。引擎加载内置目录时报告重复字段。每个条目现使用一个规范字段。
- WebSocket 缺少 HTTP `status` 的结构化参数错误会被丢弃。HTTP 专用容量分类绕过显式禁重试响应头。对应终态和重试回归见 [CORE-001](CORE-001-reserved-wait-schema-history.md) 和 [CORE-007](CORE-007-stream-recovery-visibility.md)。
- 分页新会话已有 SQLite 元数据时，历史可能仍由实时写入器持有。读取器仅对确认尚未落盘的独立空会话返回空首屏，继承历史及故障记录继续执行原有校验。
- 根据已合并的 Rust 定义重新生成 JSON、TypeScript、稳定和实验预计算产物及 Python SDK 类型。CLI 测试夹具使用 `aicodex` 构建目标，测试登录、配置和数据库使用各自的临时目录。

## 验收

- `codex-app-server-protocol --lib` 343 项通过，1 个生成入口按设计忽略。JSON、TypeScript、稳定和实验导出的一致性检验通过。
- `codex-api`、`codex-protocol`、`codex-diagnostics` 和 `codex-response-debug-context` 共 712 项通过，覆盖模拟 SSE、WebSocket 和 TLS。
- 175 个 Cargo 工作区成员的锁定依赖解析通过，Bazel 锁文件校验和目标查询通过。Python SDK 全套 309 项通过，38 项真实服务用例未启用。TypeScript SDK 构建、lint、54 项测试通过。MCP 一致性夹具 239 项通过，官方基线检查无新增失败或缺失项。
- `cargo check --workspace --tests --locked --offline` 通过。合并后 macOS ARM64 CLI 返回 `codex-cli 0.2.11`。通过本机模拟服务验收新会话及合成旧会话两次进程重启恢复，核对出站等待工具字段类型、派生 ID、历史工具输出和既有 rollout 字节。HTTP 200 流内参数错误仅发送一次请求，保留原始错误原因。
- 10:08 的 macOS ARM64 调试构建中，gzip 和 zstd 各通过 9 项包内验收，覆盖 CLI、独立 app-server、随包执行器的本机沙箱策略和 Python SDK 代码执行。测试夹具用有限次数轮询等待运行会话完成后再检查退出码。gzip 首轮超时及重跑均已记录，两种归档解压后的对应负载哈希一致。后续普通压缩事件顺序和重试遥测修订已重建并执行源码回归。
- 全工作区 nextest 主跑执行 25,629 项，25,567 通过、62 失败、80 跳过。按测试名纳入全部 62 项失败及相邻回归后重新构建，聚焦矩阵执行 748 项，746 通过。守护进程启动夹具在矩阵中超时，使用同一构件单独复跑通过。V8 ABI 防护仍失败。
- V8 `reads_typed_array_contents_with_the_compiled_binding_size` 使用校验过 SHA-256 的 152.2.0 macOS ARM64 沙箱发布资产，在合并前 `dev` 上复现同样的失败，两个父提交锁定同一资产。原生库存储 1 至 64 字节的数组为内联，Rust 绑定声明的上限为 0。防护断言在读取内容前失败。Bazel 源码替代构建停在外部依赖解析，未进入编译。发布验收需要匹配的静态库和 Rust 绑定重新运行该用例。
- 真实服务、签名发布、安装后和非本机平台属于单独的验收边界。
