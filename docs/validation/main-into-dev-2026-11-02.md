# main 合入 dev：Release 验收记录（2026-11-02）

## 范围与状态

本次将 `main` 的 `563fe7ef23a95d682f248f8c962ad399e7528cc7` 合入原
`dev` 的 `c0ba4b93df4918b7f840d832599969093dc29ae0`。本报告记录合并工作树的
已完成验收及其对应产物，不将已被替代的失败结果混入通过数量。SDK、package
和 Node 的已通过结果针对初始 Release 产物；后续核心关闭修复后的最终 binary
尚待重新构建与复验，不能将这些结果视为最终源码已通过运行验收。

验收主机为 **macOS ARM64（Darwin arm64）**。本次 Rust 验收命令均使用
**release profile**。下列 Rust 全量测试门禁尚待补充，
因此本报告当前不宣称所有合并验收已经完成。

| 项目                                    | 状态                                  |
| --------------------------------------- | ------------------------------------- |
| Release workspace binaries 构建         | 初始产物通过；最终产物待复验          |
| 初始 Release CLI 的 Python SDK 默认全套 | 283 通过，38 跳过；最终产物待复验     |
| gzip 完整包 smoke                       | 初始产物 10 通过，2 跳过；待复验      |
| zstd 完整包 smoke                       | 初始产物 10 通过，2 跳过；待复验      |
| Node 包装器退出码及信号                 | 初始产物 1 通过，无跳过；待复验       |
| Python 发布、安装、构建脚本             | 205 通过；另 perf 脚本 14 通过        |
| 仓库静态约束、Prettier、完整格式门禁    | 通过                                  |
| Rust release nextest                    | **Pending**，由主验收任务补充         |
| Rust release doctests                   | **Pending**，由主验收任务补充         |
| Rust release Clippy                     | 通过，`-D warnings`，无 warning/error |

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
  采用自己的 auth scope，并补充 `dual_header`/`assertion_only` 隔离回归。
- 从合并后的协议重新生成 stable/experimental schema 与 Python SDK，SDK
  本机默认运行时路径指向 `target/release/aicodex`，仍优先接受 `CODEX_EXEC_PATH`。
- `models_refresh_background` 使用共享 join future，并在锁外等待任务退出，
  确保并发 shutdown 或首次 shutdown 调用被取消后仍释放资源；对应新增回归
  尚待运行，Clippy 通过不代替运行时验证。
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
后续核心关闭修复后的最终产物构建与运行复验尚待补充。
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
不应成为仓库依赖约定；合并提交前需确认锁文件恢复 manifest 的固定 Git 来源。
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

## Release Clippy

最新修复后的 workspace/tests Clippy **通过，退出码 0，无 warning/error**，
耗时 **1 分 52 秒**。使用隔离 target 目录
`/tmp/aicodex-merge-clippy-target`，设置 `RUSTC_WRAPPER=`，避免其他编译任务的
wrapper/cache 干扰；验收 wrapper 同时将固定 V8 path 配置传给 Cargo 子命令。

```sh
RUSTC_WRAPPER= CARGO_TARGET_DIR=/tmp/aicodex-merge-clippy-target \
  python3 /tmp/aicodex-merge-cargo.py \
  /tmp/aicodex-merge-release-clippy-3.log \
  clippy --release --workspace --tests --keep-going --jobs 8 -- -D warnings
```

证据：`/tmp/aicodex-merge-release-clippy-3.log`。此检查包含新关闭回归测试的
静态编译检查；不代表这些测试已经运行通过。

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

下表为各套件最终有效结果；格式化后的定向复验不重复计数。

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

以下门禁均通过：

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
`/tmp/aicodex-merge-fmt-after-clippy.log`。Clippy 修复后的最新完整格式命令
退出码为 0，成功无输出；
覆盖 Just、全 Git Rust 文件、Bazel、Python SDK 与脚本格式。

另通过 release workflows YAML 与 63 个 Bash step 的语法检查、复合 action
YAML 与 6 个 Bash step 的语法检查、修改脚本的 Bash/Node 语法检查及 scoped
`git diff --check`。README 没有 TOC markers，TOC 检查按工具设计跳过，未改 README。
合并进行期间未运行 `check-clean-worktree` 或清理工作树。

## 验收边界与待补结果

- Rust release nextest、doctests 的最终命令、数量及日志尚待主验收任务补充。
- 核心关闭修复后的最终 Release binary、SDK/package/Node 复验尚待补充。
- TypeScript SDK 默认全套的两项超时诊断及固定最终 binary 复验尚待完成。
- 未使用真实上游 provider；SDK/package 的服务交互使用本机 mock。
- 未在 Windows 或 Linux 原生主机执行测试；平台 fixture/subtests 不能代替原生验收。
- 未签名、notarize、上传、发布或验证正式安装分发渠道。
- `/tmp` 路径为本次本机证据位置，不保证跨机器或长期保留；该文档记录的是已核对的
  命令结果及适用范围。
