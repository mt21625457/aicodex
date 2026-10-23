use codex_protocol::openai_models::ModelInfo;

/// 只有输出上限实际依赖输入预算时才执行历史扫描和工具序列化估算。
pub(super) fn clamp_with_lazy_input(
    model: &ModelInfo,
    estimate_input: impl FnOnce() -> i64,
) -> Option<u64> {
    // 必须使用 resolved_context_window，保留 max_context_window 回退；零或负窗口
    // 也继续交给原来的饱和运算处理，不能以“窗口必须为正”替代既有规则。
    let needs_input = model
        .max_output_tokens
        .is_some_and(|requested| requested != 0)
        && model.resolved_context_window().is_some();
    // 没有上限、上限为零或没有窗口时，原 clamp 不读取输入估算。统一调用原方法，
    // 分别保留 None、Some(0) 和原 requested 值，而不是在这里重新实现上限算法。
    model.clamped_max_output_tokens(needs_input.then(estimate_input))
}
