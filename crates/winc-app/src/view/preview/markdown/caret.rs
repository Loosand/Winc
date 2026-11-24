//! Markdown 光标与输入法组字的绘制。

use super::{App, Focus, Frame, Point, Rect};

/// 画可编辑预览中的文字光标（左边缘 `x`、所在那一行的上边缘 `y` 和高度 `h`，窗口坐标），
/// 或输入法正在组字的文字；返回输入法候选框应对齐的矩形。
///
/// 正文中的组字文字已排入所在的行（`font` 为 `None`），只返回候选框位置；代码块和表格中组字覆盖在
/// 光标处绘制，字体字号同光标处的文字（`Some(None)`：无文字时用等宽字体，字号见 [`super::line_box`]）。
pub(super) fn draw_live_caret(
    app: &App,
    frame: &mut Frame<'_>,
    (x, y, h): Point,
    font: Option<Option<(winc_render::FontKind, winc_render::Faux, f32)>>,
    clip: Rect,
) -> Rect {
    let p = app.palette;
    let preedit = app.preedit_at(Focus::Editor);
    let Some(font) = font.filter(|_| !preedit.is_empty()) else {
        if !preedit.is_empty() {
            return Rect::new(x, y, 2.0, h);
        }
        if app.blink.visible {
            let bar = Rect::new(x.round() - 1.0, y + 1.0, 2.0, h - 2.0);
            frame.rect_clipped(bar, p.caret, 1.0, clip);
        }
        return Rect::new(x, y, 2.0, h);
    };
    let (font, faux, size) = font.unwrap_or((
        winc_render::FontKind::Mono,
        winc_render::Faux::default(),
        h / 1.4,
    ));
    // 光标的高度和基线均按 [`super::line_box`] 由字号计算。
    let baseline = y + size * 1.05;
    let width = frame.measure_faux(font, faux, size, preedit);
    frame.rect_clipped(Rect::new(x, y, width, h), p.editor_bg, 0.0, clip);
    frame.text_faux(font, faux, size, x, baseline, preedit, p.text, clip);
    frame.rect_clipped(
        Rect::new(x, baseline + 3.0, width, 1.0),
        p.accent,
        0.0,
        clip,
    );
    Rect::new(x + width, y, 2.0, h)
}
