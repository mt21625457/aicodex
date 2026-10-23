# 性能复核工具

这些工具只使用生成的本地输入；不读取真实 API 凭据，不请求真实模型服务。
在 CPU 空闲、没有 Cargo 构建时串行执行。不要把局部计数比例写成整应用加速比例。

## 三个真实组件路径

先由主流程集中构建相关 Rust crate，再生成探针；此工具本身不执行 Cargo：

```sh
python3 scripts/perf/components.py --build-only \
  --baseline daf61e7a3d08c004cce53df531094a69795da819 \
  --output /tmp/aicodex-components
python3 scripts/perf/components.py --run-only --rounds 5 \
  --output /tmp/aicodex-components
```

`--deps` 可指向其他相同架构的依赖目录，`--rlib crate=/absolute/path.rlib`
可固定产物。默认选择现有产物；目录探针会通过实际 trait 编译检查
`codex_protocol` 与 `serde_json` 的兼容性。找不到匹配时失败，不退回模拟模型。

- **目录**：提取指定 Git 提交和当前 `models-manager/src/lib.rs` 的真实目录函数，
  使用各自的 `models.json`。比较两版选中 `ModelInfo` 完全相等，测 20,000 次暖查询。
- **Viewer**：编译指定提交和当前完整 `viewer.rs`，仅将两份资源 include 路径改为
  对应版本资产的绝对路径。1 MiB 片段暖命中 200 次，两版生成文件逐字相等，
  每版暖命中不能改写输出文件 mtime。
- **历史**：编译两版完整 `lookup_index.rs`，真实创建 200 万行 UTF-8/CRLF 文件，
  持文件共享锁查询尾行。冷查确认完整扫描，再测 100 次暖查并断言返回正文相同。
  此负载专门度量长尾；不代表所有 offset 均加速。自适应 stride 对长文件前缀查询
  存在更大扫描间隔的取舍，功能与随机查询由 crate 回归测试覆盖。

`manifest.json` 记录基线提交、生产源码/资产 hash、探针模板 hash、依赖路径、
编译器、平台和二进制 hash。`results.json` 保存每一轮原始样本，before/after 顺序交替。
各探针源码保存在 output 目录，便于确认测量的确切代码。

指标定义：

- `requested_bytes` / `allocation_calls`：测量区间 Rust 全局分配器的累计申请字节／调用，
  包括 realloc；不是 RSS，也不是整个应用的分配量。
- `extra_peak_live_bytes`：测量区间相对初始存活分配的最高增量。它不是绝对堆峰值，
  不包含 allocator 保留页，也不能代替进程最大 RSS。
- `interval_cpu_ns`：`getrusage(RUSAGE_SELF)` 在暖操作区间的 user+system 差值，
  排除了文件生成、冷扫描和目录首次初始化。
- `interval_wall_ns` 和 `latency_p50_ns` / `latency_p95_ns`：带分配计数器的组件执行时间。
  原子计数本身有开销；这不是未插桩应用的延迟。
- `process_user_s` / `process_system_s` / `process_max_rss_bytes`：父进程通过 wait4
  取得单个探针进程的指标，**包括初始化、等价断言和文件准备**。例如历史输入生成
  本身可能决定整个进程 RSS 峰值，所以峰值 RSS 未降低不代表暖查询没有改善。

目前 runner 支持 macOS/Linux，Windows 会明确拒绝执行；Windows 实机指标未验收。
探针用 `rustc -O` 编译，但复用现有依赖的编译设置，不能视为产品 release 构建。

## 完整本地 app-server

`local_app_server.py` 通过标准输入输出驱动完整 CLI 的 app-server，模型服务是
127.0.0.1 上的固定 SSE fixture。它记录完整子进程 CPU、最大 RSS、初始化与 turn 延迟。
使用相同配置构建的两版 CLI，交替运行多轮；启动后用户提示和模型输出保持一致。
具体比较字段及归一化规则以脚本和结果为准。它不采集完整进程的累计分配量。

此实验覆盖本地请求准备、历史和协议路径，不覆盖真实模型等待、网络、WS、
TUI 交互、全负载服务器或 Windows。它与组件探针回答不同的问题，不应合并百分比。

本轮两版 CLI 均采用 `cargo build --locked --offline -p codex-cli -p codex-v8-poc
--features codex-v8-poc/sandbox -j6`，从 `codex-rs` 执行；构建环境通过仓库
`scripts.dev_cargo.resolve_v8_env` 解析，设置 `RUST_MIN_STACK=33554432`。
基线源码用 `git archive daf61e7a3d08c004cce53df531094a69795da819` 导出到临时目录；
构建后先复制基线二进制，再构建并复制当前版本。不要在两个版本间混用构建 profile，
也不要同时运行两套大型 Cargo 构建。本轮是默认 dev 配置，没有启用 release。

```sh
python3 scripts/perf/local_app_server.py \
  --before /tmp/aicodex-perf-implementation/aicodex-baseline \
  --after /tmp/aicodex-perf-implementation/aicodex-optimized \
  --rounds 7 --turns 40 --text-bytes 16384 --output /tmp/aicodex-app-perf
```

每轮新建隔离状态目录，只向子进程传递非凭据环境白名单。请求 JSON 的归一化字段
清单和 SHA256 与输出摘要保存在样本中。首次启动异常同样保留，不能为得出更快结论删除。

## 惰性 token 预算

`lazy_tokens.py` 从基线及当前源码提取实际预算调用、历史估算、JSON 字节计数和
`ResponsesApiTools` 序列化实现。使用 512 个真实文本 ResponseItem 与 64 个工具，
计时前检查 20 组预算边界等价，计时中检查每次返回值和估算/序列化次数。
输入传入 `black_box`，避免把未使用预算场景折叠成编译期固定返回值。

```sh
python3 scripts/perf/lazy_tokens.py --build-only --output /tmp/aicodex-lazy-tokens
python3 scripts/perf/lazy_tokens.py --run-only --rounds 5 --operations 50000 \
  --output /tmp/aicodex-lazy-tokens
```

如果 target 目录包含多套 feature 图，默认最新 rlib 可能缺少 `raw_value` 或与
protocol 不兼容。此时显式传递 `--rlib serde=路径`、`--rlib serde_json=路径`、
`--rlib codex_protocol=路径`；失败日志不会被当作性能结果。本轮使用的准确产物见
`docs/design/performance-evidence-2026-10-23/lazy-manifest.json`。

`unused` 没有输出上限；`required` 有输出上限和 context window，两者都要保留。
只测输出预算阶段，不包含 Prompt 克隆、完整请求准备和真实请求所需的工具序列化。
计数为估算目的进行的序列化次数；其他真实请求序列化仍然存在。
无用路径的耗时可能低于计时器分辨率，P50 为 0 不意味着真实调用没有任何成本。

## Shell 缓存候选差分

`shell_cache.py` 从指定 Git 基线与当前 `core/src/client_common.rs` 提取真实 shell
输出配对、缓存和格式转换函数，复用现有 Rust 依赖，不运行 Cargo。它保留五个方案：
`before`（基线）、`direct`（关闭缓存）、`bypass_large`（大集合全绕过）、
`read_only_large`（大集合只读已有缓存）、`after`（当前有限接纳实现）。
后三个对照方案只改缓存策略，格式构造和配对仍来自生产源码；所有输出必须逐项等于
关闭缓存的基线 oracle，包括输出顺序和无效 JSON 的原文。

```sh
python3 scripts/perf/shell_cache.py --build-only \
  --baseline daf61e7a3d08c004cce53df531094a69795da819 \
  --output /tmp/aicodex-shell-repro
# 等待其他构建和测试结束后，单独测量已固定的二进制。
python3 scripts/perf/shell_cache.py --run-only --rounds 5 --iterations 400 \
  --output /tmp/aicodex-shell-repro
```

支持 `--deps` 和重复的 `--rlib crate=/absolute/path.rlib`。构建时先做 trait 兼容
检查，再执行一个实际 Tokio 多线程 runtime 内的缓存写入/读取断言；后者用于排除
依赖图混用导致缓存静默禁用，不采集性能。`manifest.json` 记录完整基线提交、
生产源码、探针源码、模板、runner、依赖 rlib 和二进制 hash。`--run-only` 校验
二进制 hash，不重新提取当前工作区；测量结果对应 manifest 中的代码。

可用 `--variants before after` 或 `--scenarios warm16 warm17` 选择子集。
完整矩阵包含暖缓存 16/17/32/128 个独立 8 KiB 输出、空缓存起步 32 个独立输出、
32 个重复有效/无效输出，以及 1000 个不满足缓存尺寸条件的小输出。`cold32`
只在第一次 replay 前为空缓存，后续 replay 连续复用缓存；`duplicate32` 和
`invalid32` 同样积累命中，不是每次清空缓存的冷启动测试。暖场景先依次接纳所有
输出，让各缓存方案从相同的最后 16 个条目开始。`direct` 始终不缓存。

`json_parses` 统计真实 `serde_json::from_str` 的调用次数；`warm16` 对缓存方案
额外断言零解析。每次操作包含历史 `clone`、真实配对与格式转换、完整 `assert_eq`
等价比较，所以 P50/P95/P99 和区间 CPU 不是纯转换耗时，也不是网络请求延迟。
分配计数窗口还包括同进程 Tokio 后台线程；不能归因成精确的单函数分配。
`extra_peak_live_bytes`、累计申请字节和整个进程 RSS 的区别同上，进程指标还包括
oracle、输入与缓存预热。原子解析/分配计数器本身有成本。

有限接纳改善大于容量的顺序扫描，同时允许新重复值进入缓存；大集合只读方案
不能从空缓存学习。完全关闭缓存可能在大量独立输出（例如 128 个）时更快，
因为它省去内容散列与锁操作；因此必须保留整个矩阵，不只展示 17 个输出的收益。
这些是本地历史准备路径的证据，不能据此推断整应用 CPU 或请求延迟下降同样比例。

`mixed16` 额外覆盖 16 个真实 shell 输出混合 17 个无关大工具输出的冷启动连续重放。
基线和最终实现都必须仅解析首轮的 16 次，后续全部命中。最终实现先检查正文长度上界，
只有上界超过 16 时才借用 call_id 核对配对，找到第 17 个真实候选后停止。
这会增加少量临时集合分配和配对扫描，结果不能只报告解析减少而忽略这些成本。

## 验证工具本身

```sh
python3 -m unittest discover -s scripts/perf -p 'test_*.py' -v
```

这些回归覆盖二进制 hash 变化、持续通知绕过超时、无效/不等价样本的发布、子进程中断回收
和 mock 服务启动失败清理。组件与惰性预算的 `--run-only` 会校验实际执行文件的 SHA256；
原始 manifest 和历史样本应保留，源码或探针改变后重新构建到新的输出目录再测量。
