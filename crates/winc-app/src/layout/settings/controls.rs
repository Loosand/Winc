//! 设置各行控件的几何：控件在行中的位置、步进器的各部分、主题缩略图。

use super::*;

/// 返回设置卡片在窗口 `window` 中的位置：从中央移动 `offset`，四周至少留出 `SETTINGS_MARGIN`。
pub fn settings_card(window: Rect, (dx, dy): (f32, f32)) -> Rect {
    let w = SETTINGS_W.min(window.w - SETTINGS_MARGIN * 2.0).max(0.0);
    let h = SETTINGS_H.min(window.h - SETTINGS_MARGIN * 2.0).max(0.0);
    // 窗口过小时上下限颠倒，以下限为准，不用 `clamp`。
    let place = |start: f32, span: f32, size: f32, d: f32| {
        let centered = start + (span - size) / 2.0 + d;
        centered
            .min(start + span - SETTINGS_MARGIN - size)
            .max(start + SETTINGS_MARGIN)
            .round()
    };
    Rect::new(
        place(window.x, window.w, w, dx),
        place(window.y, window.h, h, dy),
        w,
        h,
    )
}

/// 返回字体输入框 `control` 右端下拉按钮的位置。
pub(crate) fn font_menu_button(control: Rect) -> Rect {
    const WIDTH: f32 = 26.0;
    Rect::new(control.right() - WIDTH, control.y, WIDTH, control.h)
}

/// 返回模式分段控件的宽度：各段加上段间距和底槽的内边距。
pub(super) fn segmented_width() -> f32 {
    let n = SETTINGS_SEGMENT_W.len() as f32;
    let w: f32 = SETTINGS_SEGMENT_W.iter().sum();
    w + (n - 1.0) * SEGMENT_SPACING + 4.0
}

/// 返回一行中控件的位置：靠右、垂直居中。
pub(super) fn control_rect(setting: Setting, row: Rect, editing: bool) -> Rect {
    let (w, h) = match setting.control() {
        // 整行都可点击。
        SettingControl::Action | SettingControl::Search => return row,
        SettingControl::Font => (SETTINGS_CHOICE_W, SETTINGS_CONTROL_H),
        SettingControl::Command | SettingControl::Choice | SettingControl::Link => {
            (SETTINGS_CHOICE_W, SETTINGS_CONTROL_H)
        }
        SettingControl::Number => (SETTINGS_NUMBER_W, SETTINGS_CONTROL_H),
        // 产品信息铺满整行。
        SettingControl::Product => return row,
        SettingControl::Button => (SETTINGS_BUTTON_W, SETTINGS_CONTROL_H),
        SettingControl::Beta => (52.0, SETTINGS_CONTROL_H),
        SettingControl::Switch => (SETTINGS_SWITCH_W, SETTINGS_SWITCH_H),
        SettingControl::Stepper => (SETTINGS_STEP_W * 2.0 + SETTINGS_VALUE_W, SETTINGS_CONTROL_H),
        SettingControl::Themes => (segmented_width(), SETTINGS_CONTROL_H),
    };
    // 控件宽度不超过行宽减去左右内边距，并给左侧名称至少留出 80 像素。
    let w = w.min((row.w - SETTINGS_INSET * 2.0 - 80.0).max(0.0));
    // 可见内容的右缘对齐组内分隔线的右端：文字、图标类控件的底色向外多出内边距。
    let pad = match setting.control() {
        SettingControl::Choice | SettingControl::Link | SettingControl::Stepper => {
            SETTINGS_GLYPH_PAD
        }
        SettingControl::Font | SettingControl::Command if !editing => SETTINGS_GLYPH_PAD,
        _ => 0.0,
    };
    Rect::new(
        row.right() - SETTINGS_INSET + pad - w,
        row.y + (row.h - h) / 2.0,
        w,
        h,
    )
}

/// 加减控件：左右两个按钮，中间是数值。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepperParts {
    /// 左侧的减号按钮。
    pub minus: Rect,
    /// 中间显示数值的区域。
    pub value: Rect,
    /// 右侧的加号按钮。
    pub plus: Rect,
}

impl StepperParts {
    /// 根据控件的矩形 `control` 计算减号、数值和加号的位置。
    pub fn new(control: Rect) -> StepperParts {
        let minus = Rect::new(control.x, control.y, SETTINGS_STEP_W, control.h);
        let plus = Rect::new(
            control.right() - SETTINGS_STEP_W,
            control.y,
            SETTINGS_STEP_W,
            control.h,
        );
        let value = Rect::new(
            minus.right(),
            control.y,
            (plus.x - minus.right()).max(0.0),
            control.h,
        );
        StepperParts { minus, value, plus }
    }
}

/// 返回「模式」分段控件 `control` 里第 `index` 段的区域；底槽四周留 2 像素，相邻两段隔
/// `SEGMENT_SPACING`。
///
/// # Panics
///
/// `index` 不小于 3（即超出 `SETTINGS_SEGMENT_W` 的长度）时 panic。
pub fn theme_segment(control: Rect, index: usize) -> Rect {
    let before: f32 = SETTINGS_SEGMENT_W[..index].iter().sum();
    Rect::new(
        control.x + 2.0 + before + index as f32 * SEGMENT_SPACING,
        control.y + 2.0,
        SETTINGS_SEGMENT_W[index],
        control.h - 4.0,
    )
}
