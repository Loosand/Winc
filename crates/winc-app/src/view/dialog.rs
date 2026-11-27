//! 功能对话框（新建工作树、新建分支）和 Git 询问对话框的绘制。
//!
//! 淡遮罩上方是编辑区底色的卡片：标题行与关闭按钮，表单（标签、输入框、勾选框），右下角「取消」
//! 「创建」。输入框与按钮使用提交视图的样式（[`field_box`]）；名称不可用时外圈为错误色、「创建」
//! 置灰，原因显示在输入框右端（[`FormPlan`]）。`Tab` 焦点在名称以外的部件上时绘制强调色细线，
//! `git` 执行期间主按钮显示「正在创建…」，`git` 拒绝时原因以错误色显示在底部按钮左侧。

use std::borrow::Cow;
use std::f32::consts::{FRAC_PI_2, PI};

use winc_i18n::t;
use winc_render::{FontKind, Frame, Rect, Stroke};

use super::*;
use crate::app::git::form::{FormKind, FormPlan};
use crate::layout::{DialogLayout, DialogPart, QUESTION_LINE_H, REBASE_PILL_W, REBASE_ROW_H};
use crate::ui::widgets::{self, ButtonKind, ButtonSpec};
use crate::ui::{self, TextAlign};

/// 绘制对话框：遮罩，再绘制卡片（淡入途中按 [`draw_moving_card`] 整张叠上去）。
pub(super) fn draw(app: &mut App, frame: &mut Frame<'_>) {
    let layout = app.dialog_layout();
    let p = app.palette;
    match app.motion.dialog_card.motion {
        Some((true, clock)) => {
            let fade = dialog_curve(true, clock).0;
            frame.rect(app.layout.window, p.scrim.alpha(p.scrim.a * fade));
        }
        // 卡片盖住的地方不用再铺遮罩。
        _ => {
            let covered = frame.covered(layout.card, RADIUS_CARD);
            frame.rect_except(app.layout.window, p.scrim, 0.0, covered);
        }
    }
    let card = layout.card;
    draw_moving_card(app, frame, Card::Dialog, card, |app, frame| {
        draw_card_frame(frame, app.palette, card, RADIUS_CARD);
        draw_card(app, frame, &layout);
    });
}

/// 关闭后淡出期间绘制：遮罩和卡片一起淡出。
pub(super) fn draw_ghost(app: &mut App, frame: &mut Frame<'_>) {
    let p = app.palette;
    if let Some((clock, _, _)) = app.motion.dialog_card.closing() {
        let fade = dialog_curve(false, clock).0;
        frame.rect(app.layout.window, p.scrim.alpha(p.scrim.a * fade));
    }
    draw_card_ghost(app, frame, Card::Dialog);
}

/// 绘制卡片内部：标题行、几行表单、选项和底部按钮。
fn draw_card(app: &mut App, frame: &mut Frame<'_>, layout: &DialogLayout) {
    let p = app.palette;
    let Some(dialog) = &app.overlays.form_dialog else {
        return;
    };
    let plan = app.form_plan(dialog);
    let hover = dialog.hover;
    let (copy, new_window, checkout) = (dialog.copy, dialog.new_window, dialog.checkout);
    let (running, error, focus) = (dialog.running.is_some(), dialog.error.clone(), dialog.focus);
    let empty = dialog.name.text().trim().is_empty();
    // 推送对话框：当前分支、待推送的提交的标题和提交数。
    let push = match &dialog.kind {
        FormKind::Push(form) => Some((form.branch.clone(), form.subjects.clone(), form.total)),
        _ => None,
    };
    let branch = push.as_ref().map_or("", |(branch, ..)| branch.as_str());
    let (title, labels, placeholder) = match dialog.kind {
        FormKind::Push(_) => (
            t!("git-push-form-title", branch = branch),
            [
                t!("git-push-form-remote"),
                t!("git-push-form-branch"),
                t!("git-push-form-commits"),
            ],
            branch,
        ),
        FormKind::Worktree => (
            t!("common-dialog-new-worktree").to_owned(),
            [
                t!("common-dialog-branch"),
                t!("common-dialog-base"),
                t!("common-dialog-location"),
            ],
            t!("common-dialog-worktree-placeholder"),
        ),
        FormKind::Branch { .. } => (
            t!("common-dialog-new-branch").to_owned(),
            [t!("common-dialog-name"), t!("common-dialog-base"), ""],
            t!("common-dialog-branch-placeholder"),
        ),
        FormKind::Question(..)
        | FormKind::Remote { .. }
        | FormKind::GoToRevision
        | FormKind::CompareRevision { .. }
        | FormKind::RenameGroup
        | FormKind::Property(_)
        | FormKind::LogPath => {
            return draw_question(app, frame, layout);
        }
        FormKind::Clone => return draw_clone(app, frame, layout),
        FormKind::Rebase(_) => return draw_rebase(app, frame, layout),
    };
    let card = layout.card;
    frame.rounded(card, p.editor_bg, RADIUS_CARD);

    render_dialog_header(app, frame, layout.header, &title, hover);
    render_right_labels(app, frame, &layout.labels, labels);

    draw_field(
        app,
        frame,
        layout.branch,
        DialogPart::Branch,
        &plan,
        placeholder,
    );

    // 基于：能选时右端一对上下小箭头，悬停铺浅色；签出已有的分支时没有起点可选，字是淡的。
    let base = layout.base;
    field_box(app, frame, base, p.editor_bg, false, false);
    let area = base.inset(DIALOG_FIELD_INSET, 0.0);
    let color = if plan.base_enabled {
        if hover == Some(DialogPart::Base) {
            frame.rounded(base, p.hover, RADIUS_ROW);
        }
        let arrows = (area.right() - 3.0, base.y + base.h / 2.0);
        icons::up_down(frame, arrows, p.text_dim, card);
        p.text
    } else {
        p.text_dim
    };
    let text = truncate_end(frame, &plan.base, app.fonts.ui, (area.w - 20.0).max(0.0));
    ui_text(frame, app, area.x, base, &text, color, card);

    if let Some((rect, change)) = layout.location {
        draw_location(app, frame, layout, rect, change, &plan, empty);
    }

    for &(part, row) in &layout.options {
        let (text, on) = match (part, push.is_some()) {
            (DialogPart::Copy, true) => (t!("git-push-form-tags"), copy),
            (DialogPart::NewWindow, true) => (t!("common-dont-ask"), new_window),
            (DialogPart::Copy, false) => (t!("common-dialog-copy-ignored"), copy),
            (DialogPart::NewWindow, false) => (t!("common-dialog-open-in-new-window"), new_window),
            _ => (t!("common-dialog-checkout-after-create"), checkout),
        };
        let end = draw_check_option(app, frame, row, text, on);
        if part == DialogPart::Copy && push.is_some() {
            // 名称后面淡淡地说明范围：送出的是本地的全部标签。
            let hint = t!("git-push-form-tags-hint");
            ui_text_small(frame, app, end + 8.0, row, hint, p.text_dim, card);
        }
        if part == DialogPart::Copy && push.is_none() {
            // 名称后面淡淡地跟着要复制的（配置 `git.worktreeCopy`）。
            let globs = app
                .config
                .git_worktree_copy
                .join(t!("common-list-separator"));
            let room = (row.right() - end - 8.0).max(0.0);
            let globs = ellipsize(frame, &globs, app.fonts.ui_small, room);
            ui_text_small(frame, app, end + 8.0, row, &globs, p.text_dim, card);
        }
    }

    // 待推送的提交：每个标题一行，没有列出的写明个数。
    if let Some((_, subjects, total)) = &push {
        let rest = match (subjects.is_empty(), total - subjects.len().min(*total)) {
            (true, 0) => Some(t!("git-push-form-nothing").to_owned()),
            (_, 0) => None,
            (_, more) => Some(t!("git-push-confirm-more", count = more)),
        };
        let list = ui::vstack()
            .w_full()
            .h_full()
            .overflow_hidden()
            .children(subjects.iter().map(|subject| {
                let text = ellipsize(frame, subject, app.fonts.ui_small, layout.message.w);
                ui::text::<()>(text.into_owned())
                    .w_full()
                    .h(QUESTION_LINE_H)
                    .text_size(app.fonts.ui_small)
                    .line_height(QUESTION_LINE_H)
                    .color(p.text_muted)
            }))
            .when_some(rest, |col, rest| {
                col.child(
                    ui::text::<()>(rest)
                        .w_full()
                        .h(QUESTION_LINE_H)
                        .text_size(app.fonts.ui_small)
                        .line_height(QUESTION_LINE_H)
                        .color(p.text_dim),
                )
            });
        let _ = ui::render(
            frame,
            ui::text_basis(app.fonts.ui, app.metrics.ui),
            layout.message,
            list,
        );
    }

    let (confirm, enabled) = match running {
        true if push.is_some() => (t!("git-push-form-pushing"), false),
        true => (t!("common-dialog-creating"), false),
        false => (plan.confirm, plan.enabled),
    };
    render_dialog_footer(
        app,
        frame,
        layout,
        error.as_deref(),
        confirm,
        enabled,
        hover,
    );

    // 键盘焦点（经 `Tab` 移入）在名称以外的部件上时，部件外绘制一圈强调色细线；
    // 名称框有焦点时由输入框自行绘制外圈。
    let ring = match focus {
        DialogPart::Branch | DialogPart::Close => None,
        DialogPart::Base => Some(layout.base),
        DialogPart::Change => layout.location.map(|(_, change)| change),
        DialogPart::Cancel => Some(layout.cancel),
        DialogPart::Confirm => Some(layout.confirm),
        part => (layout.options.iter())
            .find(|(p, _)| *p == part)
            .map(|&(_, row)| Rect::new(row.x - 1.0, row.y + row.h / 2.0 - 7.0, 14.0, 14.0)),
    };
    if let Some(rect) = ring {
        focus_outline(
            frame,
            rect.inset(-2.0, -2.0),
            RADIUS_ROW + 2.0,
            p.accent,
            card,
        );
    }
}

/// 绘制 `git`、`ssh` 的输入请求：标题行、说明文字、只读信息、输入框和底部按钮。
fn draw_question(app: &mut App, frame: &mut Frame<'_>, layout: &DialogLayout) {
    let p = app.palette;
    let Some(dialog) = &app.overlays.form_dialog else {
        return;
    };
    let Some(view) = dialog.kind.view() else {
        return;
    };
    let (hover, focus, plan) = (dialog.hover, dialog.focus, app.form_plan(dialog));
    let (running, error) = (dialog.running.is_some(), dialog.error.clone());
    let remember = dialog.checkout;
    let card = layout.card;
    frame.rounded(card, p.editor_bg, RADIUS_CARD);

    render_dialog_header(app, frame, layout.header, &view.title, hover);

    if let Some(message) = &view.message {
        let area = layout.message;
        let msg = ui::vstack().w_full().h_full().overflow_hidden().child(
            ui::text::<()>(message.as_str())
                .w_full()
                .text_size(app.fonts.ui)
                .line_height(QUESTION_LINE_H)
                .color(p.text_muted)
                .wrap(),
        );
        let _ = ui::render(
            frame,
            ui::text_basis(app.fonts.ui, app.metrics.ui),
            area,
            msg,
        );
    }

    let labels = (view.values.iter().map(|v| v.label)).chain(view.inputs.iter().map(|i| i.label));
    render_right_labels(app, frame, &layout.labels, labels);
    for (rect, value) in layout.values.iter().zip(&view.values) {
        let area = rect.inset(DIALOG_FIELD_INSET, 0.0);
        if value.mono {
            super::git::log::mono(app, frame, (area.x, area), &value.text, p.text, area);
        } else {
            let text = ellipsize(frame, &value.text, app.fonts.ui, area.w);
            ui_text(frame, app, area.x, area, &text, p.text, area);
        }
    }

    let fields = [
        (layout.branch, DialogPart::Branch),
        (layout.second, DialogPart::Second),
        (layout.third, DialogPart::Third),
    ];
    for (input, (rect, part)) in view.inputs.iter().zip(fields) {
        draw_field(app, frame, rect, part, &plan, input.placeholder);
    }
    for &(_, row) in &layout.options {
        let check = widgets::checkbox(p, app.fonts.ui, t!("git-question-remember"), remember, ());
        let _ = ui::render(
            frame,
            ui::text_basis(app.fonts.ui, app.metrics.ui),
            row,
            check,
        );
    }
    let enabled = plan.enabled && !running;
    render_dialog_footer(
        app,
        frame,
        layout,
        error.as_deref(),
        plan.confirm,
        enabled,
        hover,
    );
    let ring = match focus {
        DialogPart::Cancel => Some(layout.cancel),
        DialogPart::Confirm => Some(layout.confirm),
        DialogPart::Checkout => (layout.options.first())
            .map(|&(_, row)| Rect::new(row.x - 1.0, row.y + row.h / 2.0 - 7.0, 14.0, 14.0)),
        _ => None,
    };
    if let Some(rect) = ring {
        focus_outline(
            frame,
            rect.inset(-2.0, -2.0),
            RADIUS_ROW + 2.0,
            p.accent,
            card,
        );
    }
}

/// 绘制交互式变基的对话框：标题行、已推送提交的提醒、提交列表、信息框和底部按钮。
fn draw_rebase(app: &mut App, frame: &mut Frame<'_>, layout: &DialogLayout) {
    let p = app.palette;
    let Some(dialog) = app.overlays.form_dialog.as_ref() else {
        return;
    };
    let FormKind::Rebase(form) = &dialog.kind else {
        return;
    };
    let form = form.clone();
    let (hover, focus, plan) = (dialog.hover, dialog.focus, app.form_plan(dialog));
    let card = layout.card;
    frame.rounded(card, p.editor_bg, RADIUS_CARD);
    render_dialog_header(app, frame, layout.header, t!("git-rebase-title"), hover);
    if form.pushed > 0 {
        let warning = t!("git-rebase-pushed-warning", count = form.pushed);
        ui_text_small(
            frame,
            app,
            layout.message.x,
            layout.message,
            &warning,
            p.warning,
            card,
        );
    }

    // 列表：处理方式、短提交号、说明；选中的行铺底色，丢弃的行文字变淡。
    let list = layout.list;
    let focused_list = focus == DialogPart::RebaseList;
    for (i, row) in form.rows.iter().enumerate() {
        let y = list.y + i as f32 * REBASE_ROW_H - drawn_scroll(form.scroll);
        let band = Rect::new(list.x, y, list.w, REBASE_ROW_H);
        if band.bottom() < list.y || band.y > list.bottom() {
            continue;
        }
        if i == form.selected {
            let fill = if focused_list { p.selection } else { p.hover };
            frame.rect_clipped(band, fill, RADIUS_ROW, list);
        } else if hover == Some(DialogPart::RebaseRow(i)) {
            frame.rect_clipped(band, p.hover, RADIUS_ROW, list);
        }
        let dropped = row.action == winc_editor::git::RebaseAction::Drop;
        let color = if dropped { p.text_dim } else { p.text };
        let pill = Rect::new(band.x, band.y, REBASE_PILL_W, band.h);
        if hover == Some(DialogPart::RebaseAction(i)) {
            frame.rect_clipped(pill, p.hover, RADIUS_ROW, list);
        }
        let label = crate::app::git::rebase::action_label(row.action);
        let accent = row.action != winc_editor::git::RebaseAction::Pick && !dropped;
        let label_color = if accent { p.accent } else { color };
        ui_text(frame, app, pill.x + 8.0, pill, label, label_color, list);
        let arrow = (pill.right() - 12.0, pill.y + pill.h / 2.0);
        icons::chevron(frame, arrow, 7.0, icons::Direction::Down, p.text_dim, list);
        let oid_x = pill.right() + 8.0;
        let oid: String = row.commit.oid.chars().take(7).collect();
        let end = super::git::log::mono(app, frame, (oid_x, band), &oid, p.text_muted, list);
        let text_x = end + 10.0;
        let subject = match (&row.message, row.action) {
            (Some(message), winc_editor::git::RebaseAction::Reword) => {
                message.lines().next().unwrap_or_default()
            }
            _ => row.commit.subject.as_str(),
        };
        let room = (band.right() - 8.0 - text_x).max(0.0);
        let subject = ellipsize(frame, subject, app.fonts.ui, room);
        // 压缩、修正的提交并入上一行，说明的颜色淡一层；丢弃的提交加删除线。
        let merged = matches!(
            row.action,
            winc_editor::git::RebaseAction::Squash | winc_editor::git::RebaseAction::Fixup
        );
        let subject_color = if merged { p.text_muted } else { color };
        let end = ui_text(frame, app, text_x, band, &subject, subject_color, list);
        if dropped {
            let strike = Rect::new(text_x, band.y + band.h / 2.0, (end - text_x).max(0.0), 1.0);
            frame.rect_clipped(strike, p.text_dim, 0.0, list);
        }
    }

    // 信息框：只在改写时可以编辑。
    let rewording = form.rewording();
    let field_focused = app.focus == crate::app::Focus::Dialog && focus == DialogPart::Branch;
    let rect = layout.branch;
    field_box(
        app,
        frame,
        rect,
        p.editor_bg,
        field_focused && rewording,
        false,
    );
    let area = rect.inset(DIALOG_FIELD_INSET, 6.0);
    if let Some(mut dialog) = app.overlays.form_dialog.take() {
        let focused = field_focused && rewording;
        let wrap = super::fields::wrap_area_field(app, frame, area.w, &mut dialog.name, focused);
        let placeholder = t!("git-rebase-message-placeholder");
        let caret = super::fields::draw_area_field(
            app,
            frame,
            area,
            &mut dialog.name,
            wrap,
            placeholder,
            focused,
        );
        app.overlays.form_dialog = Some(dialog);
        if caret.is_some() {
            app.ime.cursor = caret;
        }
    }

    // 行数超过列表高度时右边缘一条滑块，表示还有未显示的提交。
    let content = form.rows.len() as f32 * REBASE_ROW_H;
    if content > list.h && list.h > 0.0 {
        let h = (list.h * list.h / content).max(16.0);
        let y = list.y + (list.h - h) * (form.scroll / (content - list.h)).clamp(0.0, 1.0);
        let thumb = Rect::new(list.right() - 3.0, y, 3.0, h);
        frame.rect_clipped(thumb, p.text_dim.alpha(0.5), 1.5, list);
    }

    // 上移、下移：选中的行已在首尾时变淡。
    let last = form.rows.len().saturating_sub(1);
    for (rect, direction, part, enabled) in [
        (
            layout.values[0],
            icons::Direction::Up,
            DialogPart::RebaseUp,
            form.selected > 0,
        ),
        (
            layout.values[1],
            icons::Direction::Down,
            DialogPart::RebaseDown,
            form.selected < last,
        ),
    ] {
        draw_button(frame, p, rect, card, enabled && hover == Some(part), false);
        let color = if enabled {
            p.text_muted
        } else {
            p.text_dim.alpha(0.4)
        };
        icons::chevron(frame, center(rect), 9.0, direction, color, rect);
    }
    // 上移、下移按钮和底部按钮之间：不能开始的原因，否则是键盘操作的说明。
    let left = layout.values[1].right() + 12.0;
    let row = Rect::new(
        left,
        layout.cancel.y,
        (layout.cancel.x - 12.0 - left).max(0.0),
        layout.cancel.h,
    );
    let (note, color) = match &plan.problem {
        Some(problem) => (problem.clone(), p.error),
        None => {
            let alt = if cfg!(target_os = "macos") {
                "⌥"
            } else {
                "Alt+"
            };
            (t!("git-rebase-hint", alt = alt), p.text_dim)
        }
    };
    let note = ellipsize(frame, &note, app.fonts.ui_small, row.w);
    ui_text_small(frame, app, left, row, &note, color, card);
    render_dialog_buttons(app, frame, layout, plan.confirm, plan.enabled, hover);
    let ring = match focus {
        DialogPart::Cancel => Some(layout.cancel),
        DialogPart::Confirm => Some(layout.confirm),
        _ => None,
    };
    if let Some(rect) = ring {
        focus_outline(
            frame,
            rect.inset(-2.0, -2.0),
            RADIUS_ROW + 2.0,
            p.accent,
            card,
        );
    }
}

/// 绘制克隆仓库的对话框：标题行、地址、位置和底部按钮。
fn draw_clone(app: &mut App, frame: &mut Frame<'_>, layout: &DialogLayout) {
    let p = app.palette;
    let Some(dialog) = &app.overlays.form_dialog else {
        return;
    };
    let (hover, focus, plan) = (dialog.hover, dialog.focus, app.form_plan(dialog));
    let (running, error) = (dialog.running.is_some(), dialog.error.clone());
    let card = layout.card;
    frame.rounded(card, p.editor_bg, RADIUS_CARD);
    render_dialog_header(app, frame, layout.header, t!("git-clone-title"), hover);
    render_right_labels(
        app,
        frame,
        &layout.labels,
        [t!("git-clone-url"), t!("common-dialog-location")],
    );
    let placeholder = t!("git-clone-url-placeholder");
    draw_field(
        app,
        frame,
        layout.branch,
        DialogPart::Branch,
        &plan,
        placeholder,
    );
    if let Some((rect, change)) = layout.location {
        draw_location(app, frame, layout, rect, change, &plan, false);
    }
    let confirm = if running {
        t!("git-clone-running")
    } else {
        plan.confirm
    };
    let enabled = plan.enabled && !running;
    render_dialog_footer(
        app,
        frame,
        layout,
        error.as_deref(),
        confirm,
        enabled,
        hover,
    );
    let ring = match focus {
        DialogPart::Change => layout.location.map(|(_, change)| change),
        DialogPart::Cancel => Some(layout.cancel),
        DialogPart::Confirm => Some(layout.confirm),
        _ => None,
    };
    if let Some(rect) = ring {
        focus_outline(
            frame,
            rect.inset(-2.0, -2.0),
            RADIUS_ROW + 2.0,
            p.accent,
            card,
        );
    }
}

/// 沿 `rect` 绘制圆角半径为 `r` 的细线，用于表示键盘焦点。
fn focus_outline(frame: &mut Frame<'_>, rect: Rect, r: f32, color: Color, clip: Rect) {
    let r = r.min(rect.w / 2.0).min(rect.h / 2.0);
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
    let edges = [
        Stroke::Line(x0 + r, y0, x1 - r, y0),
        Stroke::Arc(x1 - r, y0 + r, r, -FRAC_PI_2, 0.0),
        Stroke::Line(x1, y0 + r, x1, y1 - r),
        Stroke::Arc(x1 - r, y1 - r, r, 0.0, FRAC_PI_2),
        Stroke::Line(x1 - r, y1, x0 + r, y1),
        Stroke::Arc(x0 + r, y1 - r, r, FRAC_PI_2, PI),
        Stroke::Line(x0, y1 - r, x0, y0 + r),
        Stroke::Arc(x0 + r, y0 + r, r, -PI, -FRAC_PI_2),
    ];
    frame.stroke(&edges, 1.5, color, clip);
}

/// 勾选框的左边与 `row` 的左边对齐，名称跟在勾选框之后。
fn draw_check_option(app: &App, frame: &mut Frame<'_>, row: Rect, text: &str, on: bool) -> f32 {
    let p = app.palette;
    let at = (row.x + 6.0, row.y + row.h / 2.0);
    let state = on.then_some(true);
    icons::checkbox(
        frame,
        at,
        state,
        p.accent,
        p.text_dim,
        p.text_on_accent,
        row,
    );
    ui_text(frame, app, row.x + 20.0, row, text, p.text, row)
}

/// 用声明式 [`widgets::dialog_header`] 绘制对话框标题栏与右上角关闭按钮。
fn render_dialog_header(
    app: &App,
    frame: &mut Frame<'_>,
    header: Rect,
    title: &str,
    hover: Option<DialogPart>,
) {
    let node = widgets::dialog_header(
        app.palette,
        app.fonts.ui,
        title,
        hover == Some(DialogPart::Close),
        DialogPart::Close,
    );
    let _ = ui::render(
        frame,
        ui::text_basis(app.fonts.ui, app.metrics.ui),
        header,
        node,
    );
}

/// 用声明式右对齐文本绘制表单名称列。
fn render_right_labels<'a>(
    app: &App,
    frame: &mut Frame<'_>,
    rects: &[Rect],
    labels: impl IntoIterator<Item = &'a str>,
) {
    let p = app.palette;
    let basis = ui::text_basis(app.fonts.ui, app.metrics.ui);
    for (&rect, label) in rects.iter().zip(labels) {
        let node = ui::text::<()>(label)
            .w_full()
            .h_full()
            .text_size(app.fonts.ui)
            .line_height(rect.h)
            .color(p.text_muted)
            .text_align(TextAlign::End);
        let _ = ui::render(frame, basis, rect, node);
    }
}

/// 用声明式 [`widgets::dialog_footer`] 绘制底部错误说明与「取消 / 确认」按钮组。
fn render_dialog_footer(
    app: &App,
    frame: &mut Frame<'_>,
    layout: &DialogLayout,
    error: Option<&str>,
    confirm: &str,
    enabled: bool,
    hover: Option<DialogPart>,
) {
    let footer_rect = Rect::new(
        layout.card.x + SETTINGS_PAD,
        layout.cancel.y,
        (layout.confirm.right() - (layout.card.x + SETTINGS_PAD)).max(0.0),
        layout.cancel.h,
    );
    let normalized = error.map(|e| Cow::Owned(e.split_whitespace().collect::<Vec<_>>().join(" ")));
    let node = widgets::dialog_footer(
        app.palette,
        (app.fonts.ui, app.fonts.ui_small),
        normalized,
        ButtonSpec {
            label: Cow::Borrowed(t!("common-cancel")),
            kind: ButtonKind::Normal,
            enabled: true,
            hovered: hover == Some(DialogPart::Cancel),
            action: DialogPart::Cancel,
        },
        ButtonSpec {
            label: Cow::Borrowed(confirm),
            kind: ButtonKind::Primary,
            enabled,
            hovered: hover == Some(DialogPart::Confirm),
            action: DialogPart::Confirm,
        },
    );
    let _ = ui::render(
        frame,
        ui::text_basis(app.fonts.ui, app.metrics.ui),
        footer_rect,
        node,
    );
}

/// 仅绘制右下角「取消 / 确认」按钮组（用于左侧已有自定义控件的变基对话框）。
fn render_dialog_buttons(
    app: &App,
    frame: &mut Frame<'_>,
    layout: &DialogLayout,
    confirm: &str,
    enabled: bool,
    hover: Option<DialogPart>,
) {
    let rect = Rect::new(
        layout.cancel.x,
        layout.cancel.y,
        (layout.confirm.right() - layout.cancel.x).max(0.0),
        layout.cancel.h,
    );
    let node = ui::hstack()
        .w_full()
        .h_full()
        .gap(8.0)
        .items_center()
        .child(widgets::action_button_with(
            app.palette,
            app.fonts.ui,
            t!("common-cancel"),
            ButtonKind::Normal,
            true,
            hover == Some(DialogPart::Cancel),
            Some(layout.cancel.w),
            DialogPart::Cancel,
        ))
        .child(widgets::action_button_with(
            app.palette,
            app.fonts.ui,
            confirm,
            ButtonKind::Primary,
            enabled,
            hover == Some(DialogPart::Confirm),
            Some(layout.confirm.w),
            DialogPart::Confirm,
        ));
    let _ = ui::render(
        frame,
        ui::text_basis(app.fonts.ui, app.metrics.ui),
        rect,
        node,
    );
}

/// 绘制 `part` 对应的输入框：有焦点时与设置中正在编辑的输入框相同，编辑区底色、外圈强调色。
///
/// 名字不可用时外圈是错误色，原因用错误色小字写在框的右端（最多占框的六成，输入的字在它左边）。
fn draw_field(
    app: &mut App,
    frame: &mut Frame<'_>,
    rect: Rect,
    part: DialogPart,
    plan: &FormPlan,
    placeholder: &str,
) {
    let p = app.palette;
    let focused = app.focus == crate::app::Focus::Dialog
        && app
            .overlays
            .form_dialog
            .as_ref()
            .is_some_and(|d| d.focus == part);
    field_box(
        app,
        frame,
        rect,
        p.editor_bg,
        focused,
        focused && part == DialogPart::Branch && plan.problem.is_some(),
    );
    let Some(mut dialog) = app.overlays.form_dialog.take() else {
        return;
    };
    let mut area = rect.inset(DIALOG_FIELD_INSET, 0.0);
    // 原因只写在第一个输入框里，第二个输入框留给输入的内容。
    if let Some(problem) = plan.problem.as_ref().filter(|_| part == DialogPart::Branch) {
        let size = app.fonts.ui_small;
        let text = ellipsize(frame, problem, size, (area.w * 0.6).round());
        let w = frame.measure(FontKind::Ui, size, &text);
        ui_text_small(frame, app, area.right() - w, rect, &text, p.error, rect);
        area.w = (area.w - w - 12.0).max(0.0);
    }
    let field = match part {
        DialogPart::Second => &mut dialog.second,
        DialogPart::Third => &mut dialog.third,
        _ => &mut dialog.name,
    };
    let caret = draw_bar_field(app, frame, area, field, placeholder, focused);
    app.overlays.form_dialog = Some(dialog);
    if caret.is_some() {
        app.ime.cursor = caret;
    }
}

/// 绘制「位置」：目录从中间截短（开头在哪、最后的分支名都保留），还没输入名字时最后一级淡淡地
/// 写「分支名称」；右端一个目录图标（选择其他目录），与「基于」右端的上下箭头颜色相同，悬停时
/// 铺浅色。
fn draw_location(
    app: &App,
    frame: &mut Frame<'_>,
    layout: &DialogLayout,
    rect: Rect,
    change: Rect,
    plan: &FormPlan,
    empty: bool,
) {
    let p = app.palette;
    field_box(app, frame, rect, p.editor_bg, false, false);
    let x = rect.x + DIALOG_FIELD_INSET;
    let area = Rect::new(x, rect.y, (change.x - 8.0 - x).max(0.0), rect.h);
    let size = app.fonts.ui;
    match &plan.dir {
        Some(dir) if empty => {
            let parent = dir.parent().unwrap_or(dir);
            let head = format!("{}/", winc_editor::display_path(parent, None));
            let tail = t!("common-dialog-branch-name");
            let room = area.w - frame.measure(FontKind::Ui, size, tail);
            let head = ellipsize(frame, &head, size, room.max(0.0));
            let end = ui_text(frame, app, area.x, rect, &head, p.text_muted, area);
            ui_text(frame, app, end, rect, tail, p.text_dim, area);
        }
        Some(dir) => {
            let text = winc_editor::display_path(dir, None);
            let text = ellipsize(frame, &text, size, area.w);
            ui_text(frame, app, area.x, rect, &text, p.text_muted, area);
        }
        None => {
            ui_text(frame, app, area.x, rect, "—", p.text_dim, area);
        }
    }

    let hovered = app
        .overlays
        .form_dialog
        .as_ref()
        .is_some_and(|d| d.hover == Some(DialogPart::Change));
    if hovered {
        frame.rounded(change, p.hover, RADIUS_ROW - 2.0);
    }
    let (cx, cy) = center(change);
    let at = ((cx - 7.5).round(), (cy - 6.0).round());
    icons::folder(frame, at, false, p.text_dim, layout.card);
}
