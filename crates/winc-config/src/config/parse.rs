//! 配置文件中各类单值与集合的 JSON 读入校验与转换。

use winc_i18n::t;

use crate::json::Json;

/// 读取字符串设置，类型不符或只有空白时向 `problems` 记录问题并返回 `None`。
pub(super) fn read_string(value: &Json, key: &str, problems: &mut Vec<String>) -> Option<String> {
    match value.as_str() {
        Some(text) if !text.trim().is_empty() => Some(text.to_string()),
        Some(_) => {
            problems.push(t!("config-empty", name = key));
            None
        }
        None => {
            problems.push(t!("config-not-string", name = key));
            None
        }
    }
}

/// 读取布尔设置，类型不符时向 `problems` 记录问题并返回 `None`。
pub(super) fn read_bool(value: &Json, key: &str, problems: &mut Vec<String>) -> Option<bool> {
    let value = value.as_bool();
    if value.is_none() {
        problems.push(t!("config-not-bool", name = key));
    }
    value
}

/// 读取数字设置并检查范围，类型不符或超出 `range` 时向 `problems` 记录问题并返回 `None`。
///
/// 整数写法同样接受（如 `"size": 14`）；`label` 用于问题文案中的名称。
pub(super) fn read_number(
    value: &Json,
    key: &str,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
    problems: &mut Vec<String>,
) -> Option<f32> {
    let Some(number) = value.as_number() else {
        problems.push(t!("config-not-number", name = key));
        return None;
    };

    if !number.is_finite() || !range.contains(&(number as f32)) {
        let setting = format!("{key} {number}");
        let range = format!("{}–{}", range.start(), range.end());
        problems.push(t!(
            "config-out-of-range",
            setting = setting,
            label = label,
            range = range
        ));
        return None;
    }

    Some(number as f32)
}
