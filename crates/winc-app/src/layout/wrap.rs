//! 按宽度折行：对话框、提示、提交信息和设置说明共用。

use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;

/// 把一段文字按宽度 `width`（逻辑像素）折成几行，返回各行的文字。
///
/// 中英文都按字符折：连着的英文字母、数字和符号算一个词，行与行之间从词边界断开，一个词
/// 比一行还长时才按字拆开。
///
/// `measure` 返回一段文字的宽度（逻辑像素）。
pub fn wrap_text(text: &str, width: f32, mut measure: impl FnMut(&str) -> f32) -> Vec<&str> {
    // 先切成不能拆开的小段：连着的英文字母、数字和符号是一个词，空格单独一段，中文按字。
    // 一行放不下时在段与段之间换行；一个词比一行还长时才按字拆开。
    // 界面字体不做字距调整：一段文字的宽度就是各段宽度之和，每段只量一次，累加出当前行的宽度。
    let mut lines = Vec::new();
    // 当前行是 `text[start..end]`，宽 `w`。
    let (mut start, mut end, mut w) = (0, 0, 0.0);
    let mut pieces = text.char_indices().peekable();
    while let Some((from, c)) = pieces.next() {
        let mut to = from + c.len_utf8();
        if c.is_ascii_graphic() {
            while let Some(&(i, next)) = pieces.peek().filter(|(_, n)| n.is_ascii_graphic()) {
                to = i + next.len_utf8();
                pieces.next();
            }
        }
        let piece = &text[from..to];
        let piece_w = measure(piece);
        if w + piece_w <= width || start == end {
            end = to;
            w += piece_w;
        } else {
            lines.push(text[start..end].trim_end());
            let rest = piece.trim_start();
            (start, end) = (to - rest.len(), to);
            w = if rest.is_empty() { 0.0 } else { piece_w };
        }
        // 一个词本身就比一行长：按字拆，每行放下尽量多的字。
        while w > width && text[start..end].chars().nth(1).is_some() {
            let (mut head, mut head_w) = (start, 0.0);
            for (i, c) in text[start..end].char_indices() {
                let c_w = measure(c.encode_utf8(&mut [0; 4]));
                if head_w + c_w > width {
                    break;
                }
                head = start + i + c.len_utf8();
                head_w += c_w;
            }
            if head == start {
                break;
            }
            lines.push(&text[start..head]);
            start = head;
            w -= head_w;
        }
    }
    lines.push(&text[start..end]);
    lines
}

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
