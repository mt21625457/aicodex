use serde::Deserialize;
use serde::Deserializer;
use serde::de::Error;
use serde_json::value::RawValue;

const INVALID_TIMEOUT: &str = "timeout_ms must be a lossless integer number of milliseconds";

/// 读取原始 JSON 数字，避免 f64 先舍入或下溢，把真正的小数误当作整数。
/// 兼容提供方的 180000.0；省略/null 仍使用默认值，上下界仍由 handler 校验。
pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Box::<RawValue>::deserialize(deserializer)?;
    if raw.get() == "null" {
        return Ok(None);
    }
    parse_integer(raw.get()).map(Some).map_err(D::Error::custom)
}

/// JSON 已验证语法；按十进制位与指数检查整除，不做浮点转换或截断。
fn parse_integer(token: &str) -> Result<i64, &'static str> {
    let negative = token.starts_with('-');
    let unsigned = token.strip_prefix('-').unwrap_or(token);
    if !unsigned.starts_with(|ch: char| ch.is_ascii_digit()) {
        return Err(INVALID_TIMEOUT);
    }
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (
            mantissa,
            exponent.parse::<i32>().map_err(|_| INVALID_TIMEOUT)?,
        ),
        None => (unsigned, 0),
    };
    let fraction_digits = mantissa
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    let digits: String = mantissa.chars().filter(|ch| *ch != '.').collect();
    let significant = digits.trim_end_matches('0');
    if significant.is_empty() {
        return Ok(0);
    }
    let trailing = i32::try_from(digits.len() - significant.len()).map_err(|_| INVALID_TIMEOUT)?;
    let fractional = i32::try_from(fraction_digits).map_err(|_| INVALID_TIMEOUT)?;
    let scale = exponent
        .checked_sub(fractional)
        .and_then(|value| value.checked_add(trailing))
        .ok_or(INVALID_TIMEOUT)?;
    if !(0..=18).contains(&scale) {
        return Err(INVALID_TIMEOUT);
    }
    // 负向累加保留 i64::MIN；所有运算检查溢出，失败直接交回工具参数错误。
    let mut value = 0_i64;
    for digit in significant.bytes() {
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_sub(i64::from(digit - b'0')))
            .ok_or(INVALID_TIMEOUT)?;
    }
    value = value
        .checked_mul(10_i64.pow(scale as u32))
        .ok_or(INVALID_TIMEOUT)?;
    if !negative {
        value = value.checked_neg().ok_or(INVALID_TIMEOUT)?;
    }
    // 保持整数 token 的 i64 范围；小数/指数表示只兼容安全整数范围。
    if (token.contains('.') || token.contains(['e', 'E']))
        && !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&value)
    {
        return Err(INVALID_TIMEOUT);
    }
    Ok(value)
}
