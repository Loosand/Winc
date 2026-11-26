//! 按宽度折行与截短：对话框、提示、提交信息和设置说明共用。

use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;

pub use winc_cpui::wrap_text;

/// 按测得的宽度从中间或末尾截短文字，超出时加上「…」。
///
/// `middle` 为真时从中间截短（开头比结尾多留一个字素），否则从末尾截短。截短按字素进行
/// （带肤色修饰或组合字符的 emoji、带声调的字母各算一个字素），不会切开字素。每个字素只测量
/// 一次宽度，结果只拼接一次。从中间截短时至少截掉一个字素，避免原文完整却多出「…」。
pub(crate) fn fit_text(
    text: &str,
    max: f32,
    middle: bool,
    mut measure: impl FnMut(&str) -> f32,
) -> Cow<'_, str> {
    if text.is_empty() || measure(text) <= max {
        return Cow::Borrowed(text);
    }
    // 每个字素在 `text` 里的字节偏移和宽度。
    let chars: Vec<(usize, f32)> = text
        .grapheme_indices(true)
        .map(|(i, g)| (i, measure(g)))
        .collect();
    let n = chars.len();
    // 保留 `keep` 个字素时，开头留 `keep - keep / 2` 个、结尾留 `keep / 2` 个；`keep` 每增加 1，
    // 偶数时开头多一个，奇数时结尾多一个，宽度随 `keep` 单调不减。
    let added = |keep: usize| match (middle, keep % 2) {
        (true, 1) => chars[n - 1 - keep / 2].1,
        (true, _) => chars[keep / 2].1,
        (false, _) => chars[keep].1,
    };
    let limit = if middle { n - 1 } else { n };
    let (mut keep, mut width) = (0, measure("…"));
    while keep < limit && width + added(keep) <= max {
        width += added(keep);
        keep += 1;
    }
    let byte = |i: usize| chars.get(i).map_or(text.len(), |c| c.0);
    let (head, tail) = if middle {
        (keep - keep / 2, keep / 2)
    } else {
        (keep, 0)
    };
    // 「…」前面不留空白（截在两个词之间时开头会以空格结尾）。
    Cow::Owned(format!(
        "{}…{}",
        text[..byte(head)].trim_end(),
        &text[byte(n - tail)..]
    ))
}
