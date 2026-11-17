# CORE-004：Cargo 复用 sccache 编译器探测失败

## 症状与根因

默认构建在 `sccache rustc -vV` 阶段报告 `Operation not permitted`，
尚未进入项目源码编译，Python traceback 是 Cargo 失败的后续传播。

本机对照：沙箱内直接探测失败；获准在沙箱外直接探测成功，但默认构建
仍返回原权限错误。只设置 `CARGO_CACHE_RUSTC_INFO=0` 后，默认构建
越过该错误进入依赖索引更新。证据表明 Cargo 的编译器信息缓存
保留了失败探测，执行权限恢复不代表缓存结果自动失效。

## 责任层与修复

`build.py` 为所有 Rust 构建 profile 设置 `CARGO_CACHE_RUSTC_INFO=0`，
每次重新探测编译器。继续强制使用 sccache，不设置 `SCCACHE_DISABLE`，
不清理 target 或共享缓存，不自动提升权限。

该修改解决旧探测错误的复用，不能赋予沙箱缺失的权限。
实际执行仍须在获准访问 sccache 的环境中进行。

## 回归与验收边界

- `python3 -m unittest scripts.test_build`：覆盖各 profile 重新探测、
  保留 sccache wrapper 和拒绝禁用 sccache。
- 本机运行对照已越过原报错；完整 release 构建及包内、安装后验收
  不由最小探测成功证明。
- 提交：通过本文件的 Git 历史查询。
