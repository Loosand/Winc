//! 编辑区绘制：行号、各类高亮、选区、语法着色和光标，只绘制可见行。
//!
//! 此外还负责图片与 SVG 的预览、无法显示文本时的说明页，以及没有打开文件时的欢迎页。

mod area;
mod caret;
mod glyphs;
mod hex;
pub(super) mod hint;
mod minimap;
mod picture;
mod placeholder;
pub(super) mod places;
mod ruler;
mod sticky;

use crate::app::HunkKind;

use area::draw_text_area;
pub(super) use caret::*;
pub(super) use glyphs::*;
pub(crate) use minimap::MinimapRows;
pub(in crate::view) use picture::*;
use placeholder::*;
pub(crate) use ruler::*;
use sticky::*;
pub(in crate::view) use sticky::{draw_pinned_rows, draw_pinned_shadow, pinned_height};

use std::fmt::Write as _;
use std::ops::{ControlFlow, Range};
use std::sync::Arc;

use ropey::RopeSlice;
use winc_core::text::{self, Columns, MAX_LINE_BYTES, MAX_LINE_CHARS};
use winc_core::{Indent, Lines, Selection, movement};
use winc_editor::PictureState;
use winc_editor::Unsupported;
use winc_i18n::t;
use winc_preview::image::Source;
use winc_syntax::{LineState, TokenKind};

use super::*;
use crate::app::{BraceKey, Focus, MARK_W, PlaceholderAction, fold_column, mark_x, ruler_rows};
use crate::layout::{GIT_LANE, MARK_LANE, Ruler, editor_content_height};
use crate::system;

/// 每帧最多向后推算高亮状态的行数（行）。
///
/// 取自调参：跳到长文件靠后的位置时推算分多帧完成，该行数与粘滞滚动的推算合计约 16 ms
/// （约一次屏幕刷新），期间界面保持响应。
pub(super) const HIGHLIGHT_BUDGET: usize = 4_000;

/// 查找匹配括号时，向前或向后最多扫描的字符数（字符）。
///
/// 超出该范围仍未找到时视为没有配对的括号，限定光标移动后单次查找的耗时。
const BRACE_SCAN_LIMIT: usize = 20_000;

/// 返回一处改动使用的颜色：仅右侧有的新增为绿，仅左侧有的删除为灰，两侧都有的修改为蓝。
pub(in crate::view) fn hunk_shade(p: &Palette, hunk: &winc_core::diff::LineHunk) -> DiffShade {
    kind_shade(p, HunkKind::of(hunk))
}

/// 返回按改动类型给出的配色。
pub(in crate::view) fn kind_shade(p: &Palette, kind: HunkKind) -> DiffShade {
    match kind {
        HunkKind::Added => p.diff_added,
        HunkKind::Deleted => p.diff_deleted,
        HunkKind::Modified => p.diff_modified,
    }
}

/// 冲突标记行上的按钮，从左到右排列；冲突序号以 `usize::MAX` 占位，绘制时替换为当前冲突的序号。
const CONFLICT_ACTIONS: [crate::app::git::merge::MergeTarget; 4] = {
    use crate::app::git::merge::MergeTarget;
    use winc_editor::git::merge::Pick;
    [
        MergeTarget::Pick(usize::MAX, Pick::Ours),
        MergeTarget::Pick(usize::MAX, Pick::Theirs),
        MergeTarget::Pick(usize::MAX, Pick::Both),
        MergeTarget::Compare(usize::MAX),
    ]
};

/// 把 [`CONFLICT_ACTIONS`] 中的占位序号替换为第 `i` 段冲突。
fn for_conflict(
    target: crate::app::git::merge::MergeTarget,
    i: usize,
) -> crate::app::git::merge::MergeTarget {
    use crate::app::git::merge::MergeTarget;
    match target {
        MergeTarget::Pick(_, pick) => MergeTarget::Pick(i, pick),
        MergeTarget::Compare(_) => MergeTarget::Compare(i),
        other => other,
    }
}

/// 返回合并冲突操作按钮的文案。
pub(in crate::view) fn merge_label(target: crate::app::git::merge::MergeTarget) -> &'static str {
    use crate::app::git::merge::MergeTarget;
    match target {
        MergeTarget::Pick(_, pick) | MergeTarget::All(pick) => pick.label(),
        MergeTarget::Compare(_) => t!("git-merge-compare"),
        MergeTarget::Previous => t!("git-merge-previous"),
        MergeTarget::Next => t!("git-merge-next"),
        MergeTarget::MarkResolved => t!("git-merge-mark-resolved"),
        MergeTarget::ThreePane => t!("git-merge-three-pane"),
        MergeTarget::EditConflict(_) => t!("git-merge-edit-conflict"),
    }
}

/// 返回冲突使用的颜色：红色只用于尚未解决的冲突，与差异视图的蓝、绿、灰区分。
pub(in crate::view) fn conflict_shade(p: &Palette) -> DiffShade {
    DiffShade {
        line: p.error.alpha(0.12),
        word: p.error.alpha(0.24),
        edge: p.error.alpha(0.45),
        mark: p.error,
    }
}

/// 返回一段已自动合并的改动使用的颜色：仅结果中有的为绿，仅该侧有的为灰，两侧都有的为蓝。
pub(in crate::view) fn merged_shade(
    p: &Palette,
    segment: &winc_editor::git::merge::AlignSegment,
) -> DiffShade {
    if segment.side.is_empty() {
        p.diff_added
    } else if segment.file.is_empty() {
        p.diff_deleted
    } else {
        p.diff_modified
    }
}

/// 返回冲突中一行（非标记行）的底色；`line` 不在冲突的内容中时返回 `None`。
fn conflict_tint(
    p: &Palette,
    conflict: &winc_editor::git::merge::Conflict,
    line: usize,
) -> Option<Color> {
    use crate::app::git::merge::{Part, part_of};
    match part_of(conflict, line)? {
        Part::Ours => Some(p.diff_modified.line),
        Part::Theirs => Some(p.diff_added.line),
        Part::Base => Some(p.diff_deleted.line),
        Part::Marker => None,
    }
}

/// 第 `line` 行所在的冲突是否已折叠。
fn conflict_folded(app: &App, line: usize) -> bool {
    (app.editor.active()).is_some_and(|doc| doc.folds().is_collapsed(line))
}

/// 绘制结果一栏中折叠为一行的第 `i` 段冲突：序号以及「保留两者」「手动编辑」。
fn draw_folded_conflict(
    app: &App,
    frame: &mut Frame<'_>,
    i: usize,
    count: usize,
    band: Rect,
    hits: &mut Vec<(Rect, crate::app::git::merge::MergeTarget)>,
) {
    use crate::app::git::merge::MergeTarget;
    use winc_editor::git::merge::Pick;
    let p = app.palette;
    // 覆盖折叠行原有的文字，再铺上与行号栏、连接带相同的底色。
    frame.rect_clipped(band, p.editor_bg, 0.0, band);
    frame.rect_clipped(band, conflict_shade(p).line, 0.0, band);
    let title = t!("git-merge-conflict-title", index = i + 1, count = count);
    let mut x = ui_text_small(frame, app, band.x + TEXT_PAD, band, &title, p.text, band) + 14.0;
    let h = (band.h - 4.0).max(12.0);
    for target in [
        MergeTarget::Pick(i, Pick::Both),
        MergeTarget::EditConflict(i),
    ] {
        let text = merge_label(target);
        let w = frame.measure(FontKind::Ui, app.fonts.ui_small, text) + 12.0;
        let rect = Rect::new(x, band.y + (band.h - h) / 2.0, w, h);
        if app.pointer.hover == Hover::Merge(target) {
            frame.rect_clipped(rect, p.hover, RADIUS_ROW, band);
        }
        ui_text_small(frame, app, rect.x + 6.0, band, text, p.accent, band);
        hits.push((rect.intersect(band), target));
        x = rect.right() + 2.0;
    }
}

/// 在 `band`（文字区中该行的范围，逻辑像素）上绘制第 `i` 段冲突（共 `count` 段）的标记行 `line`。
#[allow(clippy::too_many_arguments)]
fn draw_conflict_marker(
    app: &App,
    frame: &mut Frame<'_>,
    i: usize,
    count: usize,
    conflict: &winc_editor::git::merge::Conflict,
    line: usize,
    band: Rect,
    hits: &mut Vec<(Rect, crate::app::git::merge::MergeTarget)>,
) {
    if app.merge_panes_placed() && line == conflict.start && conflict_folded(app, line) {
        return draw_folded_conflict(app, frame, i, count, band, hits);
    }
    let p = app.palette;
    let side = |who: &str, name: &str| {
        if name.is_empty() {
            who.to_owned()
        } else {
            format!("{who} · {name}")
        }
    };
    let (color, title) = if line == conflict.start {
        (
            p.diff_modified.word,
            side(t!("git-merge-side-current"), &conflict.ours_label),
        )
    } else if Some(line) == conflict.base {
        (p.diff_deleted.word, t!("git-merge-side-base").to_owned())
    } else if line == conflict.middle {
        (
            p.diff_added.word,
            side(t!("git-merge-side-incoming"), &conflict.theirs_label),
        )
    } else {
        // `>>>>>>>`：覆盖标记文字，只绘制一道收尾的细线。
        frame.rect_clipped(band, p.editor_bg, 0.0, band);
        let rule = Rect::new(band.x, (band.y + band.h / 2.0 - 1.0).round(), band.w, 2.0);
        frame.rect_clipped(rule, p.diff_added.word, 0.0, band);
        return;
    };
    frame.rect_clipped(band, color, 0.0, band);
    let mut x = ui_text_small(frame, app, band.x + TEXT_PAD, band, &title, p.text, band) + 14.0;
    if line != conflict.start {
        return;
    }

    // 「采用当前」「采用对方」「两者都要」「对比」按钮只显示文字，悬停时铺底色。
    let h = (band.h - 4.0).max(12.0);
    for action in CONFLICT_ACTIONS {
        let target = for_conflict(action, i);
        let text = merge_label(target);
        let w = frame.measure(FontKind::Ui, app.fonts.ui_small, text) + 12.0;
        let rect = Rect::new(x, band.y + (band.h - h) / 2.0, w, h);
        if app.pointer.hover == Hover::Merge(target) {
            frame.rect_clipped(rect, p.hover, RADIUS_ROW, band);
        }
        ui_text_small(frame, app, rect.x + 6.0, band, text, p.accent, band);
        hits.push((rect.intersect(band), target));
        x = rect.right() + 2.0;
    }

    // 右端显示冲突序号。
    let counter = format!("{} / {count}", i + 1);
    let w = frame.measure(FontKind::Ui, app.fonts.ui_small, &counter);
    let right = band.right() - 14.0 - w;
    if right > x + 8.0 {
        ui_text_small(frame, app, right, band, &counter, p.text_muted, band);
    }
}

/// 编辑区绘制的结果：由上层绘制的非编辑器视图（笔记库、开始页、差异、日志）交回调用方分发。
pub(super) enum PaneView {
    Drawn,
    Library,
    Start,
    Diff(usize),
    Log(usize),
}

/// 绘制编辑区：当前标签是文本时绘制文本区（见 `area`）。
///
/// 当前标签是图片、SVG、Markdown 的预览或无法显示的文件时，改为绘制相应的预览或说明；
/// 笔记库主页、开始页、差异和日志视图交由调用方继续绘制。
pub(super) fn draw_editor(app: &mut App, frame: &mut Frame<'_>) -> PaneView {
    // Markdown、PDF 预览绘制时重新填充；预览不显示时若仍保留，滚轮和点击事件会被预览接收。
    app.preview.markdown.area = Rect::default();
    app.preview.json.clear_hits();
    app.preview.pdf.area = Rect::default();
    app.preview.pdf.page = None;
    app.preview.pdf.strip = Rect::default();

    if app.library_shown() {
        app.preview.picture_cache = None;
        return PaneView::Library;
    }

    let Some(index) = app.editor.active else {
        // 没有标签：释放缩放后的预览缓存，避免继续占用内存。
        app.preview.picture_cache = None;
        // 没有打开目录时显示开始页（打开、新建、最近目录）；打开了目录但没有标签时显示
        // 欢迎页（常用操作及其快捷键）。
        if app.editor.workspace.is_none() {
            return PaneView::Start;
        }
        draw_welcome(app, frame);
        return PaneView::Drawn;
    };

    match app.editor.documents[index].picture.clone() {
        Some(PictureState::Ready(picture)) => {
            let id = app.editor.documents[index].id;
            let area = app.layout.editor;
            draw_picture(app, frame, id, &picture, area);
            return PaneView::Drawn;
        }
        // 仍在后台解码时留空，解码完成后重新绘制。
        Some(PictureState::Loading) => return PaneView::Drawn,
        Some(PictureState::Failed(reason)) => {
            app.preview.picture_cache = None;
            let message = t!("editor-preview-failed", reason = reason);
            draw_placeholder(
                app,
                frame,
                message,
                &[(PlaceholderAction::ShowSource, t!("editor-show-source"))],
            );
            return PaneView::Drawn;
        }
        None => {}
    }
    // docx、EPUB：显示转换成 Markdown 后排版的阅读视图；读取或转换失败时说明原因。
    if (app.editor.documents[index].unsupported.as_ref())
        .is_some_and(Unsupported::reads_as_markdown)
    {
        match app.preview.markdown.failed.clone() {
            Some(reason) => {
                use PlaceholderAction::*;
                let message = t!("editor-preview-failed", reason = reason);
                let actions = [
                    (Retry, t!("editor-retry")),
                    (OpenWithDefaultApp, t!("editor-open-with-default-app")),
                ];
                draw_placeholder(app, frame, message, &actions);
            }
            None => {
                let area = app.layout.editor;
                super::preview::markdown::draw(app, frame, area);
            }
        }
        return PaneView::Drawn;
    }
    // 阅读标签：内容取回之前提示正在读取，未能取回时说明并给出重试和在浏览器中查看的按钮，
    // 取回后显示排版后的 Markdown。
    if let Some(reading) = app.editor.documents[index].reading.clone() {
        match reading {
            // 本地笔记库关闭后，旧主页不再指向任何目录。
            winc_editor::Reading::Library => {}
            winc_editor::Reading::Loading => {
                let name = app.editor.documents[index].name().into_owned();
                draw_placeholder(app, frame, t!("editor-reading", name = name), &[]);
            }
            winc_editor::Reading::Failed => {
                use PlaceholderAction::*;
                let name = app.editor.documents[index].name().into_owned();
                let actions = [
                    (Retry, t!("editor-retry")),
                    (OpenInBrowser, t!("editor-open-in-browser")),
                ];
                draw_placeholder(
                    app,
                    frame,
                    t!("editor-reading-failed", name = name),
                    &actions,
                );
            }
            winc_editor::Reading::Ready(_) => {
                let area = app.layout.editor;
                super::preview::markdown::draw(app, frame, area);
            }
        }
        return PaneView::Drawn;
    }
    // PDF：逐页显示；打开失败时说明原因。
    if app.editor.documents[index].unsupported == Some(Unsupported::Pdf) {
        match app.preview.pdf.pages.clone() {
            Some(Err(reason)) => {
                use PlaceholderAction::*;
                let message = t!("editor-preview-failed", reason = reason);
                let actions = [
                    (Retry, t!("editor-retry")),
                    (OpenWithDefaultApp, t!("editor-open-with-default-app")),
                ];
                draw_placeholder(app, frame, message, &actions);
            }
            _ => {
                let area = app.layout.editor;
                super::preview::pdf::draw(app, frame, area);
            }
        }
        return PaneView::Drawn;
    }
    let doc = &app.editor.documents[index];
    if let Some(view) = doc.table.as_deref().filter(|_| !doc.shows_text()) {
        let workbook = doc.unsupported.is_some();
        match &view.state {
            winc_editor::TableState::Ready(_) => {
                let area = app.layout.editor;
                super::preview::table::draw(app, frame, index, area);
            }
            // 仍在后台读取时留空，读取完成后重新绘制。
            winc_editor::TableState::Loading => {}
            winc_editor::TableState::Failed(reason) => {
                use PlaceholderAction::*;
                let message = t!("editor-preview-failed", reason = reason.clone());
                let actions: &[(PlaceholderAction, &str)] = if workbook {
                    &[
                        (Retry, t!("editor-retry")),
                        (OpenWithDefaultApp, t!("editor-open-with-default-app")),
                    ]
                } else {
                    &[(ShowSource, t!("editor-show-source"))]
                };
                draw_placeholder(app, frame, message, actions);
            }
        }
        return PaneView::Drawn;
    }
    let doc = &app.editor.documents[index];
    if doc.preview_kind() == Some(winc_editor::PreviewKind::Markdown)
        && doc.preview_mode() == winc_editor::PreviewMode::Preview
    {
        let area = app.layout.editor;
        super::preview::markdown::draw(app, frame, area);
        return PaneView::Drawn;
    }
    // 分栏预览时缓存中保存的是右半部分的图片（`draw_split`），不清除。
    if app.layout.split.w <= 0.0 {
        app.preview.picture_cache = None;
    }
    if doc.preview_kind() == Some(winc_editor::PreviewKind::Json)
        && doc.preview_mode() == winc_editor::PreviewMode::Preview
        && !doc.is_pending()
    {
        super::preview::json::draw(app, frame, app.layout.editor);
        return PaneView::Drawn;
    }
    if app.editor.documents[index].diff.is_some() {
        return PaneView::Diff(index);
    }
    if app.editor.documents[index].log.is_some() {
        return PaneView::Log(index);
    }
    if app.editor.documents[index].hex.is_some() {
        hex::draw_hex(app, frame, index);
        return PaneView::Drawn;
    }
    // 文件仍在读取（仅读取耗时较长时执行到此处）：提示正在读取的文件名和读了多少。
    if app.editor.documents[index].is_pending() {
        let name = app.editor.documents[index].name().into_owned();
        match app.reading_bar(app.editor.documents[index].id) {
            Some(fraction) => draw_reading(app, frame, &name, fraction),
            None => draw_placeholder(app, frame, t!("editor-reading", name = name), &[]),
        }
        return PaneView::Drawn;
    }
    if let Some(reason) = app.editor.documents[index].unsupported.clone() {
        use PlaceholderAction::*;
        let actions: &[(PlaceholderAction, &str)] = match reason {
            Unsupported::Binary(_) => &[
                (ShowHex, t!("editor-show-hex")),
                (OpenAsText, t!("editor-open-anyway")),
                (OpenWithDefaultApp, t!("editor-open-with-default-app")),
                (Reveal, system::reveal_label()),
            ],
            Unsupported::TooLarge(_) => &[
                (ShowHex, t!("editor-show-hex")),
                (OpenWithDefaultApp, t!("editor-open-with-default-app")),
                (Reveal, system::reveal_label()),
            ],
            Unsupported::Unreadable(_)
            | Unsupported::Image
            | Unsupported::Workbook
            | Unsupported::Docx
            | Unsupported::Epub
            | Unsupported::Pdf => &[
                (Retry, t!("editor-retry")),
                (Reveal, system::reveal_label()),
            ],
        };
        draw_placeholder(app, frame, reason.to_string(), actions);
        return PaneView::Drawn;
    }

    draw_text_area(app, frame, index);
    PaneView::Drawn
}

#[cfg(test)]
mod tests;

/// 长行的着色结果，跨帧复用。
///
/// 不换行时每个可见行都要为前 `MAX_LINE_CHARS` 个字符着色，一屏数十条超长行逐帧重新分析
/// 会占去大部分帧时间。只缓存不短于 [`LONG_TOKENS_MIN`] 的行，最多 [`LONG_TOKENS_LINES`] 行，
/// 按最近使用淘汰。
#[derive(Default)]
pub(crate) struct LineTokens {
    pub(super) entries: Vec<(TokensKey, Vec<winc_syntax::Token>, winc_syntax::LineState)>,
}

/// [`LineTokens`] 的键：（文档，版本，行号，着色的字符数，行首状态，语言的标识）。
pub(super) type TokensKey = (
    winc_editor::DocumentId,
    u64,
    usize,
    usize,
    winc_syntax::LineState,
    &'static str,
);

/// 着色结果需要缓存的行的最短长度（字符）：更短的行重新分析的开销可以忽略。
pub(super) const LONG_TOKENS_MIN: usize = 1_000;

/// [`LineTokens`] 最多缓存的行数（行）：足以容纳编辑区和缩略图同时显示的长行。
pub(super) const LONG_TOKENS_LINES: usize = 512;
