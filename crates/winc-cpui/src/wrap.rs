//! 按逻辑宽度对界面文字折行（词边界优先，超长词按字符拆开）。

/// 把一段文字按宽度 `width`（逻辑像素）折成几行，返回各行的文字切片。
///
/// 中英文均按字/词折行：连续 ASCII 可见字符视为一个词，优先在词边界换行；
/// 当单个词宽度超过 `width` 时按字符拆开。`measure` 返回任意切片的逻辑像素宽度。
pub fn wrap_text(text: &str, width: f32, mut measure: impl FnMut(&str) -> f32) -> Vec<&str> {
    let mut lines = Vec::new();
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
        // 一个词本身比一行长：按字拆，每行放下尽量多的字。
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
