//! 窗口里画出来的提示框（没有系统提示框时，见 `app/lifecycle/alert.rs`）：窗口上铺一层遮罩，
//! 中间一张卡片，上面是主句和说明，下面左边是「不再询问」这样的勾选框，右边一排按钮。
//! 第一个按钮是默认按钮（↩），排在最右边、铺强调色；危险操作的按钮文字用错误色。卡片的宽度、
//! 内边距，按钮和勾选框的样式，以及出现、消失时连同遮罩一起淡入淡出，都与功能对话框
//! （`view/dialog.rs`）相同。布局与可点击区域由 [`crate::ui`] 自动计算和收集。

use super::*;
use crate::app::{AlertPart, InAppAlert};
use crate::ui::widgets::{self, ButtonKind};
use crate::ui::{self, Node};

/// 卡片的内边距（逻辑像素）。
const PAD: f32 = SETTINGS_PAD;
/// 相邻两个按钮的间距（逻辑像素）。
const BUTTON_GAP: f32 = 8.0;
/// 主句、说明一行的高度（逻辑像素）。
const LINE: f32 = 20.0;
/// 卡片的最小宽度（逻辑像素）；内容更宽时加宽，最宽与功能对话框相同（`DIALOG_W`）。
const MIN_W: f32 = 320.0;

/// 绘制窗口内的提示框（打开着时），并把自动收集的命中表记回提示框；关闭后淡出期间绘制最后一帧。
pub(super) fn draw(app: &mut App, frame: &mut Frame<'_>) {
    app.track_card(Card::Alert, app.overlays.alert.is_some());
    let Some(alert) = app.overlays.alert.as_ref() else {
        if let Some((clock, _, _)) = app.motion.alert_card.closing() {
            let fade = dialog_curve(false, clock).0;
            let scrim = app.palette.scrim;
            frame.rect(app.layout.window, scrim.alpha(scrim.a * fade));
        }
        draw_card_ghost(app, frame, Card::Alert);
        return;
    };

    let p = app.palette;
    let window = app.layout.window;
    let max_w = DIALOG_W.min(window.w - 32.0).max(160.0);
    let min_w = MIN_W.min(max_w);
    let basis = ui::text_basis(app.fonts.ui, app.metrics.ui);

    let tree = build_card(app, alert, min_w, max_w);
    let laid = ui::layout(frame, basis, (None, None), tree);
    let (width, height) = laid.root_size();
    let width = width.round();
    let height = height.round();
    let card = Rect::new(
        (window.x + (window.w - width) / 2.0).round(),
        (window.y + (window.h - height) / 2.0).round(),
        width,
        height,
    );

    let fade = match app.motion.alert_card.motion {
        Some((true, clock)) => dialog_curve(true, clock).0,
        _ => 1.0,
    };
    frame.rect(window, p.scrim.alpha(p.scrim.a * fade));
    draw_moving_card(app, frame, Card::Alert, card, |app, frame| {
        let hits = laid.paint_at(frame, (card.x, card.y), window);
        if let Some(alert) = app.overlays.alert.as_mut() {
            alert.hits = hits;
        }
    });
}

/// 构造提示框卡片的声明式界面树。
fn build_card(app: &App, alert: &InAppAlert, min_w: f32, max_w: f32) -> Node<'static, AlertPart> {
    let p = app.palette;
    let (size, small) = (app.fonts.ui, app.fonts.ui_small);

    let text_block = ui::vstack()
        .w_full()
        .gap(6.0)
        .child(
            ui::text(alert.message.clone())
                .w_full()
                .text_size(size)
                .line_height(LINE)
                .color(p.text)
                .wrap(),
        )
        .when(!alert.detail.is_empty(), |col| {
            col.child(
                ui::text(alert.detail.clone())
                    .w_full()
                    .text_size(small)
                    .line_height(LINE - 2.0)
                    .color(p.text_muted)
                    .wrap(),
            )
        });

    // 按钮顺序：第一个在最右边，其余依次向左（`hstack_rev`）。
    let buttons = ui::hstack_rev().gap(BUTTON_GAP).flex_shrink(0.0).children(
        alert.buttons.iter().enumerate().map(|(i, button)| {
            let hovered = alert.hover == Some(AlertPart::Button(i));
            let kind = if i == 0 && button.key.is_none() {
                ButtonKind::Primary
            } else if button.destructive {
                ButtonKind::Destructive
            } else {
                ButtonKind::Normal
            };
            widgets::action_button(
                p,
                size,
                button.title.clone(),
                kind,
                hovered,
                AlertPart::Button(i),
            )
        }),
    );

    let checkbox = alert
        .suppression
        .as_ref()
        .map(|label| widgets::checkbox(p, size, label.clone(), alert.checked, AlertPart::Checkbox));

    let footer = ui::hstack()
        .w_full()
        .items_center()
        .justify_between()
        .gap(BUTTON_GAP * 2.0)
        .child(checkbox.unwrap_or_else(ui::spacer))
        .child(buttons);

    ui::vstack()
        .min_w(min_w)
        .max_w(max_w)
        .p(PAD)
        .gap(PAD)
        .bg(p.editor_bg)
        .rounded(RADIUS_CARD)
        .card_frame(p.card_frame())
        .child(text_block)
        .child(footer)
}
