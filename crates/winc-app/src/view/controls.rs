//! 设置和开始页共用的控件的绘制：点击后弹出菜单的值、开关、分段控件的底槽和凸起的一段。
//!
//! 这里只绘制，控件的位置、状态和点击区域由调用方决定。

use super::*;

/// 绘制点击后弹出菜单的值：平时是一段右对齐的淡色文本（未选择时 `dim` 为真，颜色更淡），
/// 其后一对上下小箭头表示可选；悬停时铺悬停底色。
///
/// 不绘制边框，样式与字体名等其他设置项一致。
pub(in crate::view) fn popup_value(
    app: &App,
    frame: &mut Frame<'_>,
    rect: Rect,
    text: &str,
    dim: bool,
    hovered: bool,
    clip: Rect,
) {
    let p = app.palette;
    if hovered {
        frame.rect_clipped(rect, p.hover, RADIUS_ROW, clip);
    }
    let area = Rect::new(
        rect.x + 10.0,
        rect.y,
        (rect.w - 10.0 - SETTINGS_GLYPH_PAD).max(0.0),
        rect.h,
    );
    let arrows = (area.right() - 3.0, rect.y + rect.h / 2.0);
    icons::up_down(frame, arrows, p.text_dim, clip);

    let room = Rect::new(area.x, area.y, (area.w - 16.0).max(0.0), area.h);
    let text = ellipsize(frame, text, app.fonts.ui, room.w);
    let w = frame.measure(FontKind::Ui, app.fonts.ui, &text);
    let color = if dim { p.text_dim } else { p.text_muted };
    ui_text(frame, app, room.right() - w, room, &text, color, room);
}

/// 绘制开关 `track`：圆角底色，开启时为强调色、圆钮在右侧，关闭时为灰色、圆钮在左侧。
///
/// `t` 是开启程度：0 为关闭，1 为开启，切换途中在两者之间，圆钮和强调色随之过渡。
pub(in crate::view) fn switch(frame: &mut Frame<'_>, p: &Palette, track: Rect, t: f32, clip: Rect) {
    let radius = track.h / 2.0;
    frame.rect_clipped(track, p.switch_off, radius, clip);
    if t > 0.0 {
        frame.rect_clipped(track, p.accent.alpha(p.accent.a * t), radius, clip);
    }
    let knob = track.h - 4.0;
    let left = track.x + 2.0;
    let x = left + (track.right() - 2.0 - knob - left) * t;
    let knob = Rect::new(x, track.y + 2.0, knob, knob);
    frame.rect_clipped(knob, p.switch_knob, knob.h / 2.0, clip);
}

/// 返回 `fg` 叠在不透明的 `bg` 上得到的颜色，结果不透明。
///
/// 用于绘制与下层完全相同的底色，以遮盖其上的文本。
pub(in crate::view) fn over(bg: Color, fg: Color) -> Color {
    let mix = |b: f32, f: f32| b + (f - b) * fg.a;
    Color {
        r: mix(bg.r, fg.r),
        g: mix(bg.g, fg.g),
        b: mix(bg.b, fg.b),
        a: 1.0,
    }
}

/// 分段控件里凸起那一段的底色和细边：浅色配色是白底、淡黑边；深色配色是比底槽 `track` 亮一档的
/// 灰、淡白边。
///
/// 都是不透明色，叠在底槽上不会透出下面。
pub(in crate::view) fn segment_colors(p: &Palette, track: Color) -> (Color, Color) {
    let white = Color::hex(0xFFFFFF);
    if p.dark {
        let raised = over(track, white.alpha(0.09));
        (raised, over(track, white.alpha(0.14)))
    } else {
        (white, over(track, Color::hex(0x000000).alpha(0.08)))
    }
}

/// 绘制分段控件的底槽 `track` 和其中凸起的一段 `slot`（没有选中项时为 `None`），各段的文字由
/// 调用方压在上面。`under` 是控件下面的底色，凸起段的颜色据此算出。
pub(in crate::view) fn segment_track(
    frame: &mut Frame<'_>,
    p: &Palette,
    track: Rect,
    under: Color,
    slot: Option<Rect>,
    clip: Rect,
) {
    frame.rect_clipped(track, p.hover, RADIUS_ROW + 2.0, clip);
    let track_bg = over(under, p.hover);
    let (raised, edge) = segment_colors(p, track_bg);
    if let Some(slot) = slot {
        frame.rect_clipped(slot, edge, RADIUS_ROW, clip);
        frame.rect_clipped(slot.inset(0.5, 0.5), raised, RADIUS_ROW - 0.5, clip);
    }
}

/// 侧栏列表、文件树与 Git 视图共用的行外观常量与辅助函数。
pub(in crate::view) mod list {
    use crate::view::*;

    /// 标签栏或列表边缘内容被裁掉时，渐隐区域的宽度或高度（逻辑像素）。
    pub(in crate::view) const TAB_FADE: f32 = 24.0;
    /// 放不下的名称在侧栏边缘淡出的宽度（逻辑像素），与结果行上按钮左侧的渐隐同宽。
    pub(in crate::view) const NAME_FADE: f32 = 20.0;
    /// 项目按钮里徽章占的宽度（逻辑像素）：左边 6、徽章 20、右边 8。
    pub(in crate::view) const BADGE_ROOM: f32 = 34.0;
    /// 徽章中心相对项目按钮左边的距离（逻辑像素）。
    pub(in crate::view) const BADGE_CENTER: f32 = 16.0;
    /// 名字收回时，末尾的淡出从过渡的这个进度开始出现（经验值）：名字已基本缩回侧栏。
    pub(in crate::view) const FADE_FROM: f32 = 0.7;
    /// 合并显示的目录之间分隔符的不透明度（经验值）：比目录名淡，一眼能分出各级。
    pub(in crate::view) const CHAIN_SEPARATOR: f32 = 0.45;
    /// 合并显示的目录之间分隔符两侧各留的空白（逻辑像素，经验值）。
    pub(in crate::view) const CHAIN_GAP: f32 = 4.0;
    /// 合并显示的目录放不下时代替被折叠的各级的记号。
    pub(in crate::view) const CHAIN_FOLD: &str = "…";

    /// 返回名字收回途中这一行的底色：选中的行不变，其余是悬停的底色随进度 `progress` 淡出。
    pub(in crate::view) fn retracting_color(
        p: &Palette,
        selected: bool,
        focused: bool,
        progress: f32,
    ) -> Option<Color> {
        row_color(p, selected, focused, false).or_else(|| {
            row_color(p, selected, focused, true)
                .map(|color| color.alpha(color.a * (1.0 - progress)))
        })
    }

    /// 折叠放不进 `room` 的目录链，返回从左到右要显示的各部分（`Some` 是目录的下标，`None`
    /// 是一处折叠）和总宽度。
    ///
    /// `widths` 是各级名字的宽度，`step` 是相邻两部分之间分隔符和留白的宽度，`fold` 是折叠记号
    /// 的宽度。第一级和当前的一级 `current` 始终显示，最后一级放得下就显示；还有空间时从离
    /// `current` 近的各级依次补上，距离相同时先补靠后的。
    pub(in crate::view) fn fold_chain(
        widths: &[f32],
        step: f32,
        fold: f32,
        current: usize,
        room: f32,
    ) -> (Vec<Option<usize>>, f32) {
        let last = widths.len() - 1;
        let lay_out = |keep: &[bool]| {
            let mut parts = Vec::new();
            for (i, &kept) in keep.iter().enumerate() {
                if kept {
                    parts.push(Some(i));
                } else if parts.last() != Some(&None) {
                    parts.push(None);
                }
            }
            let width: f32 = parts
                .iter()
                .map(|part| part.map_or(fold, |i| widths[i]))
                .sum();
            let width = width + step * parts.len().saturating_sub(1) as f32;
            (parts, width)
        };

        let all = lay_out(&vec![true; widths.len()]);
        if all.1 <= room {
            return all;
        }
        let mut keep = vec![false; widths.len()];
        keep[0] = true;
        keep[current] = true;
        keep[last] = true;
        if lay_out(&keep).1 > room && current != last {
            keep[last] = false;
        }
        let mut order: Vec<usize> = (0..widths.len()).collect();
        order.sort_by_key(|&i| (i.abs_diff(current), std::cmp::Reverse(i)));
        for i in order {
            if keep[i] {
                continue;
            }
            keep[i] = true;
            if lay_out(&keep).1 > room {
                keep[i] = false;
                break;
            }
        }
        lay_out(&keep)
    }
}
