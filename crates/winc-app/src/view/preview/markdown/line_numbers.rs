//! 可编辑 Markdown 的源码行号。

use super::{App, FontKind, Frame, Rect, drawn_scroll};

/// 原文行号的右边离正文左边的距离（逻辑像素）：与编辑区行号栏和文字之间的间距相近。
const NUMBER_GAP: f32 = 12.0;

/// 在可编辑预览的左侧留白里写原文行号（见 `Laid::source_lines`），光标所在的行颜色深一些。
///
/// 关闭行号设置时、只读的预览中不写；留白放不下最大的行号时也不写。
pub(super) fn draw_source_numbers(app: &App, frame: &mut Frame<'_>, area: Rect) {
    if !app.markdown_line_numbers_on() || !app.markdown_live() {
        return;
    }
    let (Some(laid), Some(doc)) = (app.preview.markdown.layout.as_ref(), app.editor.active())
    else {
        return;
    };
    let Some(rope) = doc.text().rope() else {
        return;
    };
    let p = app.palette;
    let size = app.fonts.code;
    let cell = frame.measure(FontKind::Mono, size, "0");
    let col_left = laid.column_bounds().0;
    let right = col_left - NUMBER_GAP;
    if col_left - number_room(app, frame) < area.x + 4.0 {
        return;
    }
    let lit = selected_rows(laid, rope, &doc.selection);
    let top = laid.content_y(drawn_scroll(app.preview.markdown.scroll()));
    let lines = laid.source_lines(rope, top..top + area.h);
    for (line, y) in lines {
        let number = (line + 1).to_string();
        let x = right - number.len() as f32 * cell;
        // 行号只作定位参考，比正文的次要色更淡；光标或选区所在显示行的行号用次要色。
        let color = match lit.contains(line, y) {
            true => p.text_muted,
            false => p.text_dim,
        };
        let baseline = area.y + y - top;
        frame.text(FontKind::Mono, size, x, baseline, &number, color, area);
    }
}

/// 光标或选区占据的范围，决定哪些行号显示为当前行（见 [`draw_source_numbers`]）。
enum Lit {
    /// 显示行的上下范围（相对内容顶部，与 `laid::source_lines` 的基线同一量法）。
    Rows(f32, f32),
    /// 光标或选区的一端落在没有显示文字的位置（空行、分隔线）：按原文行号的范围判断。
    Lines(usize, usize),
}

impl Lit {
    /// 写在基线 `y` 处的原文第 `line` 行的行号是否在光标或选区所在的显示行上。
    fn contains(&self, line: usize, y: f32) -> bool {
        match *self {
            Lit::Rows(top, bottom) => (top..=bottom).contains(&y),
            Lit::Lines(first, last) => (first..=last).contains(&line),
        }
    }
}

/// 选区 `selection` 在排版结果 `laid` 中占据的显示行；一行原文可能折成几个显示行，几行原文也
/// 可能排在同一显示行，因此按显示行判断，不按原文行号判断。
fn selected_rows(
    laid: &crate::app::preview::markdown::laid::Laid,
    rope: &ropey::Rope,
    selection: &winc_core::Selection,
) -> Lit {
    let (from, to) = (
        selection.anchor.min(selection.head),
        selection.anchor.max(selection.head),
    );
    let byte = |c: usize| rope.char_to_byte(c.min(rope.len_chars()));
    match (laid.point_of(byte(from)), laid.point_of(byte(to))) {
        (Some((_, a, _)), Some((_, b, h))) => Lit::Rows(laid.content_y(a), laid.content_y(b + h)),
        _ => {
            let line = |c: usize| rope.char_to_line(c.min(rope.len_chars()));
            Lit::Lines(line(from), line(to))
        }
    }
}

/// 原文行号在正文左侧占用的宽度（逻辑像素）：最大行号的宽度加上与正文的间距；不写行号时为 0。
pub(super) fn number_room(app: &App, frame: &mut Frame<'_>) -> f32 {
    if !app.markdown_line_numbers_on() || !app.markdown_live() {
        return 0.0;
    }
    let Some(rope) = app.editor.active().and_then(|d| d.text().rope()) else {
        return 0.0;
    };
    let cell = frame.measure(FontKind::Mono, app.fonts.code, "0");
    rope.len_lines().to_string().len() as f32 * cell + NUMBER_GAP
}
