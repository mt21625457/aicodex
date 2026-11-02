# main 合入 dev：Release 验收记录（2026-11-02）

## 范围与状态

**构建保持暂停**：2026-11-02T21:25:23+08:00 按用户要求停止第二轮 release 构建。随后用户要求审查全部改动、修复问题并提交；本轮执行静态交叉审查和不调用编译器的检查，不恢复构建。合并已记录为本地 `9834f79ab3`，本轮修复以追加提交保留其历史。未推送，第二轮运行验收仍未完成。

用户于 2026-11-02T17:41:14+08:00 暂停任务，随后于
2026-11-02T18:54:14+08:00 要求继续。复用保留的源码、缓存与日志后，首轮完整
四 package 测试 hooks nextest 已运行结束但未通过；修复后的第二轮尚未运行。

本次将 `main` 的 `563fe7ef23a95d682f248f8c962ad399e7528cc7` 合入原
`dev` 的 `c0ba4b93df4918b7f840d832599969093dc29ae0`。本报告记录合并工作树的
已完成验收及其对应产物，不将已被替代的失败结果混入通过数量。SDK、package
和 Node 的已通过结果针对初始 Release 产物；后续修复后的最终 binary
尚待重新构建与复验，不能将这些结果视为最终源码已通过运行验收。

验收主机为 **macOS ARM64（Darwin arm64）**。本次 Rust 验收命令均使用
**release profile**。下列 Rust 全量测试门禁尚待补充，
因此本报告当前不宣称所有合并验收已经完成。

| 项目                                    | 状态                                    |
| --------------------------------------- | --------------------------------------- |
| Release workspace binaries 构建         | 初始产物通过；最终产物待复验            |
| 初始 Release CLI 的 Python SDK 默认全套 | 283 通过，38 跳过；最终产物待复验       |
| gzip 完整包 smoke                       | 初始产物 10 通过，2 跳过；待复验        |
| zstd 完整包 smoke                       | 初始产物 10 通过，2 跳过；待复验        |
| Node 包装器退出码及信号                 | 初始产物 1 通过，无跳过；待复验         |
| Python 发布、安装、构建脚本             | 205 通过；另 perf 脚本 14 通过          |
| 仓库静态约束、Prettier、完整格式门禁    | 静态约束先前通过；第二轮完整格式通过    |
| Rust release nextest                    | 首轮 23531 通过、172 失败；二轮 pending |
| Rust release doctests                   | **Pending**，由主验收任务补充           |
| Rust release Clippy（测试 hooks 配置）  | 审查前第二轮通过；新增修复未重跑        |
| Rust release Clippy（最终普通生产配置） | 审查前第二轮通过；新增修复未重跑        |

## 最终交叉审查与追加修复

本轮保持构建停止，按模块交叉检查当前未提交改动及合并整合点，并以另一位
agent 复核新发现的关键修复。与原 dev 相比的 1731 个合并路径逐一核对来源：
1263 个文件与 main 的 blob 和 mode 一致，8 个删除与 main 一致；其余 460 个
混合或本地整合路径逐模块审查双亲差异，同时核对 199 个 remerge 差异路径。
对直接继承 main 的文件采用来源一致性核验，不宣称重新逐行审计上游全部实现。

本轮新发现并修复：

- 旧 DeepSeek 会话仅覆盖 model 或 reasoning 时，保存的 provider 别名规范化被
  提前返回跳过。将规范化放入恢复保存 provider 的分支，保留显式 provider、
  未知 provider 和 managed policy 边界，补 12 场景单元用例及集成覆盖源码。
- `policy_capacity_unavailable` 被媒体错误文本启发式抢先分类，丢失结构化错误
  和 Retry-After。恢复显式错误 code 的优先级，补带/不带等待时间的回归源码。
- Seatbelt 与 CLI 测试只覆盖 `CODEX_HOME`，继承的 `AICODEX_HOME` 可能使其读取
  错误目录。同步两键，包括二次覆盖和 PTY 环境；保留原行为断言。
- SDK CI 仍寻找旧 `codex` 文件名，WinGet 包转换及 Python runtime staging/lookup
  未适配 canonical `aicodex` 入口。修复路径及 SHA manifest 映射；Python 依据
  metadata 仅接受两个 canonical 入口，保留旧官方包兼容并拒绝任意路径。
- TUI 重连错误快照遗漏原 dev 对真实时钟闪烁字符的归一化。恢复局部归一化，
  保留 main 新增错误内容、状态及时间阈值断言。
- app-server 文档误称 review 总是原线程执行；修正 dev 保留的 detached 兼容
  行为说明。恢复 Cargo.lock 固定 V8 Git 来源，避免临时验收路径进入依赖约定。

先前对模型目录刷新、启动 Auth fallback、测试隔离和快照的修复也已重新复核。
61 个 core 和 2 个 extension 快照逐条检查；两份受宿主技能污染的候选未接受，
仍需新 Release fixture 生成。6 个临时 `.snap.new` 已按原路径和 SHA256 归档于
`/tmp/aicodex-final-review-snapshot-candidates`，不进入提交。

协议静态校验：stable 预计算包的 765 个 TypeScript、322 个 JSON 与落盘一致；
3719 个 schema 本地引用均有效；24 个 protobuf/prost 字段编号一致且唯一。
本轮 Rust 新增回归只做源码及格式检查，没有编译或执行；此前 Clippy 通过结果
不覆盖这些新改动。最终运行验收仍按本文末尾待办执行。

本轮不触发编译的最终检查：`just fmt-check`、Cargo manifest 约束、TUI/Core
边界、Bazel Clippy 配置约束、README ASCII 及 `git diff --check` 全部通过；
修改文档和 workflow 通过 Prettier。发布入口修复的 16 项 Python fixture 测试、
1 项 WinGet unittest（含 x64/ARM64 子测试）与 1 项 SDK workflow unittest
共 **18 项通过**，不涉及真实编译或安装。记录为
`/tmp/aicodex-final-review-{python-runtime,winget,sdk-workflow}-tests.log`；
全仓格式日志为 `/tmp/aicodex-final-review-fmt.log`。提交前确认无 Cargo/rustc
构建进程，本项目先前暂停的 rust-analyzer 仍保持暂停，防止自动检查启动编译。

逐模块记录位于本机 `/tmp/aicodex-final-review-*.md`，来源分类为
`/tmp/aicodex-final-review-provenance.json`。这些记录用于说明本轮静态审查范围，
不替代新源码的运行验收。

## 当前修复与二轮计划

首轮完整四 package hooks nextest 编译耗时 **67 分 35 秒**，测试执行耗时
**941.474 秒**。共运行 **23703** 项：**23531 通过**（含 27 slow、5 flaky）、
**172 失败**，另有 **75 skipped**。日志：
`/tmp/aicodex-merge-release-nextest-testhooks-resumed.log`。这是未通过的完整首轮，
不得将其中通过数量与后续定向测试相加后称为最终验收通过。

多 agent 已针对共性原因修改 fixture、测试与兼容实现，尚待重新编译和完整复验。
已核对的调整包括 exec fixture 同时隔离 `CODEX_HOME`/`AICODEX_HOME`、明确
loopback 绕过代理及 HTTP SSE fixture 不支持 WebSocket；测试 SQLite 配置保留
日志目录；独立执行器分别验证有 package metadata 和源码构建的版本约定；
seatbelt 断言覆盖 `.aws` 保护路径；V8 typed-array 测试区分空 view 的合法
lazy buffer 行为并保留非空 view 的 placement、内容及指针断言。

快照已逐条审查并接受 **61 个 core + 2 个 extension** 更新；另有 **2 个受污染
候选**尚待修正后的 fixture 重新生成。快照接受不等于相关测试已复验通过。

已完成的提前定向验证单独记录如下，不替代最新源码全套验收：

- TUI 4 项通过：`/tmp/aicodex-merge-tui-resumed-four-tests.log`。
- OTel release exporter 1 项通过：`/tmp/aicodex-merge-otel-release-test.log`。
- 使用既有 release test binaries、设置 loopback `NO_PROXY` 后，code-mode-host
  **37/37** 通过：`/tmp/aicodex-code-mode-release-loopback`。
- 同样使用既有 release test binaries、loopback `NO_PROXY`，HTTP、feedback、
  core-plugins 与 RMCP 的 **9 项**定向测试全部通过：
  `/tmp/aicodex-merge-proxy-targeted`。这些复跑说明对应失败受代理环境影响，
  不证明修复后的所有源码已经验收。
- 旧 core binary 的 shell snapshot 与 Guardian 两项短 timeout 用例，在原环境、
  原阈值下串行复跑通过，分别耗时 **0.60 秒**、**33.60 秒**。这是旧 binary 的
  诊断，不计入新源码全套结果；binary SHA256 与用例日志记录于
  `/tmp/aicodex-core-early-rerun-results.json`。

第二轮计划从**同一份 nextest metadata**生成两组互斥选择：

1. A 组：`codex-code-mode-host`、`codex-http-client`、`codex-feedback`、
   `codex-core-plugins`、`codex-rmcp-client`，显式为 loopback 设置 `NO_PROXY`。
2. B 组：该 metadata 中 A 组的严格补集，保留原环境，继续验证代理相关行为。

执行前后需核对 A/B 无交集，且并集覆盖完整目标集合。当前第二轮尚未运行，
其最终统计与集合核对结果仍 pending。主 target 的第二轮
binaries 与 Cargo/nextest metadata 构建已停止，日志为
`/tmp/aicodex-merge-round2-release/build.log`。保持默认 release 的 opt-level 3、
thin LTO 与前述四 package hooks overrides；后续复用同批 binaries 和 metadata
进行定向及 A/B 完整测试；当前目录中的不完整 metadata 不能用于宣称全套通过。
审查前源码的两套第二轮 strict Clippy 均已通过，详见下节；本轮新修复不在其覆盖内。

## 关键兼容决策

- 保留 dev 的 `aicodex` 可执行文件与 npm/安装入口，并接入 main 的独立
  `codex-exec-server` 打包。CLI、app-server 两类包均包含所需执行器。
- 版本语义保持分层：`AICODEX_VERSION` 用于发布/npm 元数据；CLI 的 Clap
  banner 来自 CLI Cargo 包名及 workspace 版本。本次真实运行输出为
  `codex-cli 0.1.11`，smoke 按源码定义验证，不强行将 banner 改为发布版本。
- macOS 签名脚本、DMG 链接与复合构建 action 使用 `bin/aicodex`；保留原有
  `codex` 签名身份及 entitlement 内容，并显式映射入口名称。
- 根目录、Rust 子目录、Bazel 与 release workflow 的 Rust 工具链约定统一为
  `1.95.0`。远程 exec 启动辅助脚本默认 release，按显式 profile 选择产物路径。
- 保留 dev 的 DeepSeek 与 compaction watchdog 定制；适配 main 的 history
  模块抽取，保留 fork usage hints 与有界 role wrappers；成功完成流后才记录
  DeepSeek prefix。跨 provider 切换时清空旧 `runtime_auth`，让新 provider
  采用自己的 auth scope；`dual_header`/`assertion_only` 两项隔离回归已提前
  定向运行通过，仍待最终 suite 验收。
- 从合并后的协议重新生成 stable/experimental schema 与 Python SDK，SDK
  本机默认运行时路径指向 `target/release/aicodex`，仍优先接受 `CODEX_EXEC_PATH`。
- `models_refresh_background` 使用共享 join future，并在锁外等待任务退出，
  确保并发 shutdown 或首次 shutdown 调用被取消后仍释放资源；对应 4 项
  shutdown 回归已提前定向运行通过，仍待最终 suite 验收。
- 适配 release 测试语义：OTel 按 debug/release 分别验证默认 exporter；TUI
  测试遵循 release 隐藏 Rollout 的行为，使用独立 release 列宽快照，并将共享
  空快照替换为等价断言；保留 dev context continuation 历史行为。
- 删除已无源码、测试或仓库命令消费者的空 `legacy-claude-tests` feature；
  未添加策略例外。删除前后 `codex-core` 完整 metadata targets 数组一致：
  共 6 个 target，含 5 个 integration test target。
- 全 Git Rust 文件格式门禁覆盖独立 fixture/probe 文件；新增格式适配仅作
  Ruff/rustfmt 格式化与 import 拆分，不改变逻辑或削弱检查规则。
- pnpm 12 不再读取 `.npmrc` 中原有的四项安装设置；将 `nodeLinker: hoisted`、
  `shamefullyHoist: true`、`strictPeerDependencies: false`、
  `preferWorkspacePackages: true` 显式写入 `pnpm-workspace.yaml`，保留 `.npmrc`
  供旧 pnpm 使用。此修复恢复既定依赖布局，使 ESLint 配置可解析已锁定的
  `@eslint/js`；未改依赖版本、锁文件或 `allowBuilds` 等安全策略。

## Release 构建与 V8 本地镜像

初始 `build --release --workspace --bins` 完成，耗时 **21 分 22 秒**，产物位于
`codex-rs/target/release`。构建日志：`/tmp/aicodex-merge-release-build.log`。
后续全部修复后的最终产物构建与运行复验尚待补充。
本次 release 配置包含 `debug = "line-tables-only"`、`split-debuginfo = "off"`
及 `strip = false`，具备供本机 symbols 验收使用的调试信息。

V8 依赖采用以下临时本地路径方案完成验收：

1. manifest 固定 `rusty_v8` revision
   `350c5a859a9b3c84ab8701b3ad51e3b7699b30c0`，crate 版本 `152.2.0`。
2. 核对已有 Cargo Git checkout 的完整 HEAD 与上述 revision 一致，再用
   `git archive HEAD` 原样导出至 `/tmp/aicodex-merge-rusty-v8-350c5a8`，
   未修改导出的 V8 源码。
3. 通过仓库 `scripts/codex_package/v8.py` 获取 Codex 对应的 `152.2.0`
   macOS ARM64 预编译库与 binding，并完成 SHA256 校验；使用匹配的
   pointer-compression/sandbox release 产物。
4. 验收命令通过临时 Cargo 配置
   `patch.crates-io.v8.path="/tmp/aicodex-merge-rusty-v8-350c5a8"`，以及
   `RUSTY_V8_ARCHIVE`、`RUSTY_V8_SRC_BINDING_PATH` 指向上述本地产物。
   `/tmp/aicodex-merge-cargo.py` 对命令强制要求 `--release`。

此方法验证了固定源码版本及匹配预编译产物下的本机行为；它不证明普通远端
Git/默认产物下载路径可用，也不证明从 V8 C++ 源码全量构建通过。临时路径
不应成为仓库依赖约定；本轮已核对并恢复锁文件中的固定 Git 来源，与 manifest revision 一致。
本地配置证据：`/tmp/aicodex-merge-v8-env.json`、
`/tmp/aicodex-merge-cargo.py`。

## 协议与 Python SDK

stable 与 experimental schema fixture 的 release 生成测试均通过
（各 1 通过），日志分别为 `/tmp/aicodex-merge-schema-stable.log`、
`/tmp/aicodex-merge-schema-experimental.log`。Python SDK 生成成功，日志为
`/tmp/aicodex-merge-sdk-generate.log`。

在 `sdk/python` 使用锁定环境执行默认全套，指定本次初始 release CLI：

```sh
CODEX_EXEC_PATH="$PWD/../../codex-rs/target/release/aicodex" \
  RUN_REAL_CODEX_TESTS=0 \
  UV_CACHE_DIR=/tmp/aicodex-merge-uv-cache \
  uv run --only-group dev --frozen --no-sync pytest
```

结果 **283 通过、38 跳过**，耗时 **72.58 秒**，无 deselection。38 项跳过为
需要显式开启的真实 provider 测试；默认集成测试使用本机真实 app-server 与
mock Responses 服务。日志：`/tmp/aicodex-merge-sdk-release-test.log`。
该结果早于核心关闭修复；最终 binary 的相同全套复验仍 pending。

同一环境的 `ruff check --output-format=github .` 与 `ruff format --check .`
均通过，格式检查覆盖 74 个文件。证据：`/tmp/aicodex-merge-sdk-ruff.log`、
`/tmp/aicodex-merge-sdk-format.log`。

## 真实包与 Node 验收

使用初始构建的四个真实 release binary：`aicodex`、`codex-app-server`、
`codex-code-mode-host`、`codex-exec-server`。阶段脚本为
`/tmp/aicodex-merge-package-smoke.sh`，结果目录为
`/tmp/aicodex-merge-real-package-20261102`。

- 使用 `dsymutil` 生成四份真实 dSYM，逐一核对 Mach-O UUID，生成实际 symbols
  归档。macOS symbol smoke 通过，`nm`/`atos` 可解析 code-mode-host main。
  dsymutil 对已移除的部分 thin-LTO 原生临时归档对象发出警告，因此不声称
  所有依赖的调试符号均完整。
- `scripts/build_codex_package.py` 使用全部显式预构建 binary，指定
  `--cargo /usr/bin/false --cargo-profile release --strip none`，分别生成
  CLI 与 app-server 包的 gzip/zstd 归档，未再次编译。ripgrep 使用本机已有
  可执行文件并装入各自包内。
- 两种压缩格式均运行完整 `test_codex_package.py`：gzip **10 通过、2 跳过**
  （29.24 秒），zstd **10 通过、2 跳过**（30.15 秒）。仅跳过 Linux/Windows
  专属 symbols 测试，没有排除运行时测试。
- 覆盖 CLI help/features/completion/version、两个包的执行器初始化与
  filesystem/process sandbox、独立执行器和子命令启动错误一致性、SDK code
  mode 使用对应包内 ripgrep，以及 macOS 符号解析。
- 包装前后四个原始 release binary 的 SHA256 一致。

证据位于结果目录的 `smoke-gzip.log`、`smoke-zstd.log`、
`release-inputs.sha256`、`inputs-unchanged.log`、UUID 文件和 dSYM 日志。

```sh
CODEX_TEST_BINARY="$PWD/codex-rs/target/release/aicodex" \
  node --test codex-cli/tests/exit-status.test.mjs
```

Node 结果 **1 通过、0 跳过**，覆盖数字退出码及 SIGHUP/SIGXFSZ 转发。
日志：`/tmp/aicodex-merge-node-signal.log`。
本节包与 Node 结果均早于核心关闭修复，最终 binary 的复验仍 pending。

## Release 测试配置与 Clippy

部分既有 integration fixture 依赖 `debug_assertions` 控制的测试 hooks。
为执行这些 fixture，测试命令仅对以下四个 package 设置
`profile.release.package.<name>.debug-assertions=true`：

| Package              | Fixture 所需行为                                                 |
| -------------------- | ---------------------------------------------------------------- |
| `codex-app-server`   | 启动、managed config、user config、login URL、限流 timeout hooks |
| `codex-core-plugins` | loopback HTTP bundle                                             |
| `codex-analytics`    | capture 文件                                                     |
| `codex-arg0`         | 临时 `CODEX_HOME` 工具别名                                       |

这些是命令级 Cargo profile overrides，不修改生产 `Cargo.toml`。命令仍使用
`--release`、`opt-level=3` 与 thin LTO；其他 package 保持原 profile 设置。
带 hooks 的测试构建与普通生产 Release 产物验收分别记录，不能相互替代。

主 target 的普通 release full-nextest 尝试已完成编译，耗时 **154 分 23 秒**，
随后主动终止，**没有完成全量测试执行**，不能计为 nextest 通过。上述四 package
测试 hooks variant 的首轮完整执行结果见“当前修复与二轮计划”；修复后的最终
nextest 与 doctests 仍 pending。
提前直接运行的 core binary 得到 3008 通过、1 ignored；旧 TUI
binary 得到 5918 通过、5 个旧 expectation 失败、4 ignored。相关 expectation
已修正，并已有前述 TUI 定向复验通过；这两次旧 binary 提前探测均不计为最终
全套通过。

nextest `local` profile 继承 `default`，没有新增 `default-filter` 或 exclude；
默认 `retries = 1`。`local` 仅将 app-server integration 并发设为 4，未收窄
测试选择范围。本机执行范围是 macOS 默认非 ignored suite，不覆盖以下环境：

- 由 cfg 限制为 Windows/Linux 的测试。
- 依赖真实 remote、Docker 或 Wine 的环境测试。
- ignored 的 live API、tmux 等测试。

部分 remote skip 宏通过提前 return 跳过主体，nextest 仍会将其显示为 PASS。
因此后续报告的 PASS 数量不能等同于所有测试主体均实际执行；需结合上述条件
和测试日志解释覆盖范围。

首轮全量测试修复之前的源码在上述四 package 测试 hooks 配置下，workspace/tests strict Clippy
**通过，退出码 0，无 warning/error**，耗时 **3 分 30 秒**。检查使用
`clippy --release --workspace --tests --keep-going --jobs 4 -- -D warnings`，
验收 wrapper 为 Cargo 子命令传递固定 V8 path 配置。
证据：`/tmp/aicodex-merge-release-testhooks-clippy-final.log`。
此检查包含测试代码的静态检查，不代表测试已运行通过。

同一先前阶段源码的普通生产配置（不含四 package overrides）strict Clippy 亦已
**通过，退出码 0，无 warning/error**，耗时 **3 分 11 秒**。日志为
`/tmp/aicodex-merge-release-clippy-production-final.log`。最终普通生产 Release
须重新构建，并分别完成 Python SDK、package、Node 与 TypeScript SDK 验收；
前文初始产物结果不覆盖这一步。
上述两份日志仅保留为先前阶段证据。首轮失败修复后、最终交叉审查前的源码已完成第二轮
`clippy --release --workspace --tests --keep-going --jobs 4 -- -D warnings`，
两种配置均 **PASS，退出码 0，无 warning/error**：

- 四 package hooks 配置：`/tmp/aicodex-merge-release-clippy-round2-hooks-final2.log`。
- 普通生产配置：`/tmp/aicodex-merge-release-clippy-round2-production-final.log`，
  耗时 **1 分 43 秒**。

Clippy 通过不代替后续测试执行、普通生产 binary 重建或 SDK/package 验收。

## TypeScript SDK

检查 SDK 的 package scripts、Jest setup 与 MCP harness 后，确认下列命令不
调用 Cargo。现有 Node 依赖通过 `pnpm install --frozen-lockfile --ignore-scripts`
恢复既定 hoisted 布局；隔离布局遗留的本地 `.bin` shim 移至 `/tmp` 留存。
锁文件 SHA256 前后一致。串行执行普通命令均通过，且没有再次切换安装布局：

```sh
pnpm -r --filter ./sdk/typescript run lint
pnpm -r --filter ./sdk/typescript run build
```

构建包含 ESM、sourcemap 与 TypeScript declarations。证据：
`/tmp/aicodex-merge-ts-install-final.log`、`/tmp/aicodex-merge-ts-lint-final.log`、
`/tmp/aicodex-merge-ts-build-final.log`、`/tmp/aicodex-merge-ts-lock.sha256`、
`/tmp/aicodex-merge-ts-layout-final.sha256`。锁定的官方 MCP conformance
依赖已用 `pnpm rebuild @modelcontextprotocol/conformance` 恢复其 JavaScript
CLI，未编译 Rust，见 `/tmp/aicodex-merge-ts-conformance-build.log`。

初始 Release CLI 的默认全套预检使用显式 `CODEX_EXEC_PATH` 与 `--runInBand`，
结果为 **52 通过、2 失败**（5 个 suite 中 4 个通过），耗时 191.089 秒。
MCP conformance suite 通过；失败均为 `run.test.ts` 的原有 5 秒超时：
`passes additionalDirectories as repeated flags`、
`writes output schema to a temporary file and forwards it`。
未调大阈值或跳过测试，原因尚未确定。

该预检与 Rust 编译并行，运行中及结束时 CLI SHA256 相同，且等于初始 package
产物 SHA256；未单独记录启动前 SHA256。因此它不构成最终源码的运行验收。
待 Rust 编译结束后，须将最终 Release binary 固定复制到独立目录，再以
`CODEX_EXEC_PATH` 指向固定副本，使用 `--runInBand` 重新执行完整 suite。
证据：`/tmp/aicodex-merge-ts-test.log`、
`/tmp/aicodex-merge-ts-runtime-midrun.sha256`、
`/tmp/aicodex-merge-ts-runtime-after.sha256`。

## 脚本与仓库门禁

下表为先前阶段已完成的各套件结果；格式化后的定向复验不重复计数，不将这些
数量混入本轮 Rust 测试结果。

| 套件                                                                            | 结果                       | 本机日志                                      |
| ------------------------------------------------------------------------------- | -------------------------- | --------------------------------------------- |
| `scripts/codex_package` Python unit tests                                       | 32 通过                    | `/tmp/aicodex-merge-package-format-tests.log` |
| `scripts/install` Python unit tests                                             | 22 通过                    | `/tmp/aicodex-merge-install-format-tests.log` |
| `codex-cli/tests` Python unit tests                                             | 1 通过，含 6 平台 subtests | `/tmp/merge-npm-unit.log`                     |
| `.github/scripts` Python unit tests                                             | 81 通过                    | `/tmp/merge-github-unit.log`                  |
| `.github/scripts/macos-signing` Python unit tests                               | 13 通过                    | `/tmp/merge-signing-unit-2.log`               |
| `test_build_storage_config`、`test_dev_cargo`、`test_build_cache`、`test_build` | 56 通过                    | `/tmp/aicodex-merge-build-format-tests.log`   |
| perf 脚本 tests                                                                 | 14 通过                    | `/tmp/aicodex-merge-perf-script-tests.log`    |

上述构建脚本测试 mock Cargo 执行，缓存测试只操作临时目录；签名测试 mock
外部签名工具。它们不触发 Rust debug 构建或真实签名。

以下门禁在先前阶段均通过，最新修改的最终复核尚待补充：

```sh
python3 .github/scripts/verify_cargo_workspace_manifests.py
python3 .github/scripts/verify_tui_core_boundary.py
python3 .github/scripts/verify_bazel_clippy_lints.py
python3 scripts/asciicheck.py README.md
pnpm run format
UV_CACHE_DIR=/tmp/aicodex-merge-uv-cache just fmt-check
```

manifest 最终通过及 targets 一致性证据为
`/tmp/aicodex-merge-legacy-feature-note.md`、
`/tmp/aicodex-merge-legacy-feature-targets.json`。其他对应日志为
`/tmp/aicodex-merge-tui-boundary.log`、`/tmp/aicodex-merge-bazel-clippy.log`、
`/tmp/aicodex-merge-readme-ascii.log`、`/tmp/aicodex-merge-pnpm-format.log`、
`/tmp/aicodex-merge-fmt-release-tests.log`。该阶段包含 OTel/TUI 测试适配的完整格式命令
退出码为 0，成功无输出；
覆盖 Just、全 Git Rust 文件、Bazel、Python SDK 与脚本格式。

第二轮 `just fmt-check` 亦已 **退出码 0**，日志
`/tmp/aicodex-merge-fmt-round2.log`（成功无输出）。随后两处纯 lint 行调整已单独
通过 rustfmt；此处区分全仓格式检查时点与随后的局部格式确认。

另通过 release workflows YAML 与 63 个 Bash step 的语法检查、复合 action
YAML 与 6 个 Bash step 的语法检查、修改脚本的 Bash/Node 语法检查及 scoped
`git diff --check`。README 没有 TOC markers，TOC 检查按工具设计跳过，未改 README。
合并进行期间未运行 `check-clean-worktree` 或清理工作树。

## 验收边界与待补结果

- Rust release nextest 二轮 A/B 严格补集验收、完整集合核对，以及 doctests 尚待完成。
- 2 个污染快照候选待新 fixture 生成；审查前源码 Clippy 已通过，新增修复及完整运行验收仍待完成。
- 不含测试 hooks overrides 的最终普通生产 Release binary 重建，以及
  SDK/package/Node 复验尚待补充。
- TypeScript SDK 默认全套的两项超时诊断及固定最终 binary 复验尚待完成。
- 合并提交 `9834f79ab3` 已存在；本轮追加审查修复提交不代表完整 Release 运行验收通过。
- 未使用真实上游 provider；SDK/package 的服务交互使用本机 mock。
- 未在 Windows 或 Linux 原生主机执行测试；平台 fixture/subtests 不能代替原生验收。
- 未签名、notarize、上传、发布或验证正式安装分发渠道。
- `/tmp` 路径为本次本机证据位置，不保证跨机器或长期保留；该文档记录的是已核对的
  命令结果及适用范围。
