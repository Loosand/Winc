//! 笔记库主页：侧栏入口、目录导航和只绘制可见行的卡片网格。

use winc_i18n::t;
use winc_render::{Faux, Stroke};

use super::*;
use crate::app::workspace::library::{
    AttachmentPreview, CardContent, FolderPose, FolderPreview, Grid, JsonExcerpt, Library,
    LibraryTarget, PreviewLine,
};

pub(super) fn draw_home_button(app: &App, frame: &mut Frame<'_>) {
    let r = app.library_home_button();
    if r.w <= 0.0 || r.h <= 0.0 {
        return;
    }
    let p = app.palette;
    let on = app.library_shown() && app.library.folder.as_deref() == app.library_root();
    let hover = app.pointer.hover == Hover::Library(LibraryTarget::Root);
    draw_button(frame, p, r, app.layout.sidebar, hover, on);
    let at = (r.x + 15.0, center(r).1);
    home_icon(frame, at, if on { p.accent } else { p.text_muted }, r);
    ui_text(
        frame,
        app,
        r.x + 30.0,
        r,
        t!("workspace-library-home"),
        p.text,
        r,
    );
}

pub(in crate::view) fn draw(app: &mut App, frame: &mut Frame<'_>) {
    let area = app.layout.editor;
    let grid = Grid::new(area, &app.library.cards);
    let p = app.palette;
    let x = grid.body.x;
    let folder = app.library.folder.as_deref();
    let name = folder
        .map(|path| winc_editor::display_name(path).into_owned())
        .unwrap_or_default();
    let is_root = folder == app.library_root();
    let title = if is_root {
        t!("workspace-library-home").into()
    } else {
        name.clone()
    };
    let title_size = app.fonts.ui * 1.65;
    let title_rect = Rect::new(x, area.y + 12.0, grid.body.w, 36.0).intersect(area);
    let title = ellipsize(frame, &title, title_size, title_rect.w);
    frame.text_faux(
        FontKind::Ui,
        BOLD,
        title_size,
        x,
        title_rect.y + 26.0,
        &title,
        p.text,
        title_rect,
    );
    if is_root {
        let subtitle = Rect::new(x, title_rect.bottom(), grid.body.w, 20.0).intersect(area);
        let label = ellipsize(frame, &name, app.fonts.ui, subtitle.w);
        ui_text(frame, app, x, subtitle, &label, p.text_muted, subtitle);
    }

    let animate = app.animations_on();
    let phase = if animate {
        app.library.loading_phase()
    } else {
        0.0
    };
    if app.library.loading_visual() {
        draw_loading_status(app, frame, title_rect, animate);
    }

    if app.library.cards.is_empty() {
        if app.library.loading() {
            draw_skeleton_grid(frame, p, area, phase, animate);
            return;
        }
        let msg = if let Some(error) = &app.library.error {
            t!("workspace-library-failed", error = error.as_str())
        } else {
            t!("workspace-library-empty").to_owned()
        };
        let row = Rect::new(x, grid.body.y + 8.0, grid.body.w, 30.0).intersect(area);
        let msg = ellipsize(frame, &msg, app.fonts.ui, row.w);
        ui_text(frame, app, x, row, &msg, p.text_muted, row);
        return;
    }

    let scroll = app
        .library
        .scroll
        .min(grid.max_scroll(app.library.cards.len()));
    let folder_clip = Rect::new(area.x, grid.body.y - 6.0, area.w, grid.body.h + 6.0);
    for index in grid.visible(app.library.cards.len(), scroll) {
        let c = &app.library.cards[index];
        let rect = grid.card(index, scroll, matches!(c.content, CardContent::Folder(_)));
        let clip = rect.intersect(grid.body);
        if clip.w <= 0.0 || clip.h <= 0.0 {
            continue;
        }
        let target = LibraryTarget::Card(index);
        let hover = matches!(app.pointer.hover, Hover::Library(
            LibraryTarget::Card(i) | LibraryTarget::NewInFolder(i) | LibraryTarget::FolderMenu(i)
        ) if i == index);
        let paint_clip = if matches!(c.content, CardContent::Folder(_)) {
            rect.inset(-6.0, -6.0).intersect(folder_clip)
        } else {
            clip
        };
        let brush = CardBrush {
            p,
            clip: paint_clip,
            shadow_clip: folder_clip,
            hot: hover,
        };
        match &c.content {
            CardContent::Note(lines) => {
                brush.sheet(frame, rect, brush.edge(), 16.0);
                brush.page(frame, rect, &c.name, lines, true);
            }
            CardContent::PendingNote => {
                brush.sheet(frame, rect, brush.edge(), 16.0);
                brush.pending_page(frame, rect, &c.name, skeleton_wave(phase, index, animate));
            }
            CardContent::File | CardContent::Attachment(_) => {
                brush.sheet(frame, rect, brush.edge(), 16.0);
                brush.attachment(
                    frame,
                    rect,
                    &c.name,
                    app.library.attachment_preview(&c.path),
                    false,
                );
            }
            CardContent::Folder(info) => {
                let pose = app.library.folder_pose(index);
                brush.folder(frame, rect, &c.name, info.as_ref(), pose, &app.library);
                // Same footer controls as Quiet MD, drawn before the card hit so they win.
                let reveal = pose.flap.clamp(0.0, 1.0);
                if reveal > 0.01 {
                    let size = 28.0;
                    let y = rect.bottom() - 14.0 + pose.lift() - size;
                    let right = rect.right() - 14.0;
                    for (x, target) in [
                        (right - size * 2.0 - 8.0, LibraryTarget::NewInFolder(index)),
                        (right - size, LibraryTarget::FolderMenu(index)),
                    ] {
                        let button = Rect::new(x, y, size, size);
                        if button.x >= rect.x && button.intersect(clip).h > 0.0 {
                            brush.folder_button(frame, button, target, app.pointer.hover, reveal);
                            if hover && reveal >= 0.20 {
                                app.hits.library.push((button.intersect(clip), target));
                            }
                        }
                    }
                }
            }
        }
        app.hits.library.push((clip, target));
    }
    draw_grid_scrollbar(app, frame);
}

pub(super) fn draw_grid_scrollbar(app: &mut App, frame: &mut Frame<'_>) {
    let Some((_, track, scroll, content)) = app.card_grid_scrollbar() else {
        return;
    };
    super::draw_scrollbar(
        app,
        frame,
        crate::app::ScrollArea::CardGrid,
        0,
        track,
        scroll,
        content,
    );
}

/// 计算每张骨架卡片的呼吸亮度（`0.0..=1.0`），按卡片序号微错峰形成平缓波纹。
fn skeleton_wave(phase: f32, index: usize, animate: bool) -> f32 {
    if !animate {
        return 0.5;
    }
    0.5 + 0.5 * (phase * 3.6 - index as f32 * 0.42).sin()
}

/// 在标题行右端绘制轻量的旋转圆弧与「正在读取文件夹…」提示，避免大目录等待时空白无反馈。
fn draw_loading_status(app: &App, frame: &mut Frame<'_>, title_rect: Rect, animate: bool) {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};

    let p = app.palette;
    let label = t!("workspace-library-loading");
    let size = app.fonts.ui_small;
    let text_w = frame.measure(FontKind::Ui, size, label);
    let spinner_gap = 18.0;
    let total_w = spinner_gap + text_w;
    if total_w + 80.0 > title_rect.w {
        return;
    }
    let left = title_rect.right() - total_w;
    let cy = title_rect.y + 20.0;
    let cx = left + 5.5;
    let radius = 4.5;
    frame.stroke(
        &[Stroke::Circle(cx, cy, radius)],
        1.3,
        p.separator,
        title_rect,
    );
    let start = if animate {
        icons::spin_turn().rem_euclid(1.0) * TAU - PI
    } else {
        -FRAC_PI_2
    };
    frame.stroke(
        &[Stroke::Arc(cx, cy, radius, start, start + 2.2)],
        1.3,
        p.text_muted,
        title_rect,
    );
    let label_rect = Rect::new(left + spinner_gap, title_rect.y + 8.0, text_w + 4.0, 24.0);
    ui_text_small(
        frame,
        app,
        label_rect.x,
        label_rect,
        label,
        p.text_muted,
        title_rect,
    );
}

/// 尚未收到目录条目时，按与真实纸张卡片一致的 5:6 网格绘制带柔和呼吸动效的骨架卡片。
fn draw_skeleton_grid(frame: &mut Frame<'_>, p: &Palette, area: Rect, phase: f32, animate: bool) {
    let grid = Grid::for_cards(area, false);
    let count = (grid.cols * 2).clamp(4, 12);
    let ink = if p.dark {
        Color::hex(0xffffff)
    } else {
        Color::hex(0)
    };
    for index in 0..count {
        let rect = grid.card(index, 0.0, false);
        let clip = rect.intersect(grid.body);
        if clip.w <= 0.0 || clip.h <= 0.0 {
            continue;
        }
        let wave = skeleton_wave(phase, index, animate);
        let brush = CardBrush {
            p,
            clip,
            shadow_clip: grid.body,
            hot: false,
        };
        brush.sheet(frame, rect, p.border.alpha(0.55), 16.0);

        let margin = (rect.w * 0.075).clamp(10.0, 22.0);
        let body = rect.inset(margin, margin);
        let inner = clip.intersect(body);
        if inner.w <= 0.0 || inner.h <= 0.0 {
            continue;
        }
        let title_h = (rect.w * 0.068).clamp(11.0, 16.0);
        let title_frac = [0.64, 0.52, 0.72, 0.58][index % 4];
        let title_bar = Rect::new(body.x, body.y + 2.0, body.w * title_frac, title_h);
        frame.rect_clipped(title_bar, ink.alpha(0.075 + 0.045 * wave), 4.5, inner);
        let body_brush = CardBrush {
            p,
            clip: inner,
            shadow_clip: grid.body,
            hot: false,
        };
        body_brush.skeleton_lines(
            frame,
            body,
            title_bar.bottom() + (rect.w * 0.06).clamp(10.0, 16.0),
            rect.w,
            wave,
            index,
        );
    }
}

const BOLD: Faux = Faux {
    bold: true,
    italic: false,
};

struct CardBrush<'a> {
    p: &'a Palette,
    clip: Rect,
    shadow_clip: Rect,
    hot: bool,
}

impl CardBrush<'_> {
    fn edge(&self) -> Color {
        if self.hot {
            self.p.accent.alpha(0.65)
        } else {
            self.p.border
        }
    }

    fn sheet(&self, frame: &mut Frame<'_>, rect: Rect, edge: Color, radius: f32) {
        frame.rect_clipped(rect, edge, radius, self.clip);
        frame.rect_clipped(
            rect.inset(1.0, 1.0),
            self.p.popup_bg,
            (radius - 1.0).max(0.0),
            self.clip,
        );
    }

    fn pending_page(&self, frame: &mut Frame<'_>, rect: Rect, title: &str, wave: f32) {
        let margin = (rect.w * 0.075).clamp(10.0, 22.0);
        let body = rect.inset(margin, margin);
        let clip = self.clip.intersect(body);
        if clip.w <= 0.0 || clip.h <= 0.0 {
            return;
        }
        let title_size = (rect.w * 0.074).clamp(12.0, 20.0);
        let title_lines = crate::layout::wrap_text(title, body.w, |s| {
            frame.measure_faux(FontKind::Ui, BOLD, title_size, s)
        });
        let mut y = body.y + title_size;
        for line in title_lines.iter().take(2) {
            frame.text_faux(
                FontKind::Ui,
                BOLD,
                title_size,
                body.x,
                y,
                line,
                self.p.text,
                clip,
            );
            y += title_size * 1.18;
        }
        let body_brush = CardBrush {
            p: self.p,
            clip,
            shadow_clip: self.shadow_clip,
            hot: false,
        };
        body_brush.skeleton_lines(frame, body, y + 4.0, rect.w, wave, 0);
    }

    fn skeleton_lines(
        &self,
        frame: &mut Frame<'_>,
        body: Rect,
        start_y: f32,
        card_w: f32,
        wave: f32,
        seed: usize,
    ) {
        let ink = if self.p.dark {
            Color::hex(0xffffff)
        } else {
            Color::hex(0)
        };
        let line_h = (card_w * 0.036).clamp(6.0, 9.0);
        let stride = line_h * 2.1;
        let color = ink.alpha(0.045 + 0.035 * wave);
        let patterns: [[f32; 5]; 3] = [
            [0.92, 0.84, 0.96, 0.72, 0.48],
            [0.86, 0.95, 0.78, 0.88, 0.42],
            [0.94, 0.76, 0.90, 0.68, 0.56],
        ];
        let widths = patterns[seed % patterns.len()];
        for (i, frac) in widths.into_iter().enumerate() {
            let y = start_y + i as f32 * stride;
            if y + line_h > body.bottom() {
                break;
            }
            let bar = Rect::new(body.x, y, body.w * frac, line_h);
            frame.rect_clipped(bar, color, line_h / 2.0, self.clip);
        }
    }

    fn file(&self, frame: &mut Frame<'_>, rect: Rect, name: &str) {
        let margin = (rect.w * 0.075).clamp(10.0, 22.0);
        let body = rect.inset(margin, margin);
        let clip = self.clip.intersect(body);
        let icon = Rect::new(body.x, body.y, 32.0, 32.0);
        frame.rect_clipped(icon, self.p.hover, 8.0, clip);
        draw_file_icon(
            frame,
            self.p,
            (center(icon).0 - icons::FILE_WIDTH / 2.0, center(icon).1),
            name,
            clip,
        );
        let size = (rect.w * 0.065).clamp(12.0, 17.0);
        let lines = crate::layout::wrap_text(name, body.w, |s| {
            frame.measure_faux(FontKind::Ui, BOLD, size, s)
        });
        let mut y = icon.bottom() + 9.0 + size;
        for line in lines.iter().take(2) {
            frame.text_faux(FontKind::Ui, BOLD, size, body.x, y, line, self.p.text, clip);
            y += size * 1.18;
        }
    }

    fn attachment(
        &self,
        frame: &mut Frame<'_>,
        rect: Rect,
        name: &str,
        preview: Option<&AttachmentPreview>,
        inset: bool,
    ) {
        let Some(preview) = preview else {
            self.file(frame, rect, name);
            return;
        };
        let margin = (rect.w * 0.075).clamp(10.0, 22.0);
        let body = rect.inset(margin, margin);
        let clip = self.clip.intersect(body);
        let title_size = (rect.w * 0.065).clamp(12.0, 17.0);
        let title = ellipsize(frame, name, title_size, body.w - 3.0);
        let title_y = match preview {
            AttachmentPreview::Image(pixels) => {
                let height = if inset {
                    body.h * 0.62
                } else {
                    body.h - title_size - 12.0
                };
                let panel = Rect::new(body.x, body.y, body.w, height.max(0.0));
                frame.rect_clipped(panel, self.p.hover, 8.0, clip);
                let room = panel.inset(6.0, 6.0);
                let (w, h) = (pixels.width as f32, pixels.height as f32);
                let fit = (room.w / w).min(room.h / h);
                let (w, h) = (w * fit, h * fit);
                let (x, y) = center(room);
                frame.pixels_scaled(
                    Rect::new(x - w / 2.0, y - h / 2.0, w, h),
                    pixels.width,
                    pixels.height,
                    &pixels.rgba,
                    clip,
                );
                panel.bottom() + 9.0 + title_size
            }
            AttachmentPreview::Json(excerpt) => {
                let header = title_size + 10.0;
                let panel = Rect::new(body.x, body.y + header, body.w, (body.h - header).max(0.0));
                self.json_excerpt(frame, panel, excerpt);
                body.y + title_size
            }
        };
        frame.text_faux(
            FontKind::Ui,
            BOLD,
            title_size,
            body.x,
            title_y,
            &title,
            self.p.text,
            clip,
        );
    }

    fn json_excerpt(&self, frame: &mut Frame<'_>, panel: Rect, excerpt: &JsonExcerpt) {
        let clip = self.clip.intersect(panel);
        if clip.w <= 0.0 || clip.h <= 0.0 {
            return;
        }
        frame.rect_clipped(panel, self.p.hover, 8.0, clip);
        let text = panel.inset(9.0, 7.0);
        let clip = clip.intersect(text);
        let size = (panel.w * 0.065).clamp(8.0, 11.0);
        let stride = size * 1.5;
        let capacity = ((text.h + stride - size) / stride).floor() as usize;
        let y_at = |i: usize| text.y + size + i as f32 * stride;
        match excerpt {
            JsonExcerpt::Fields { rows, more } => {
                use crate::view::preview::json::value_text;
                let details = Rect::new(text.x + text.w * 0.54, text.y, text.w * 0.46, text.h);
                let labels = Rect::new(text.x, text.y, (details.x - text.x - 6.0).max(0.0), text.h);
                let overflow = *more || rows.len() > capacity;
                let n = capacity
                    .saturating_sub(usize::from(overflow))
                    .min(rows.len());
                for (i, row) in rows[..n].iter().enumerate() {
                    let y = y_at(i);
                    let (value, kind) = value_text(&row.value);
                    let value_area = if row.key.is_empty() { text } else { details };
                    if !row.key.is_empty() {
                        frame.text(
                            FontKind::Mono,
                            size,
                            text.x,
                            y,
                            &row.key,
                            self.p.syntax_property,
                            clip.intersect(labels),
                        );
                    }
                    let ink = kind.map_or(self.p.text_muted, |kind| self.p.token(kind));
                    frame.text(
                        FontKind::Mono,
                        size,
                        value_area.x,
                        y,
                        &value,
                        ink,
                        clip.intersect(value_area),
                    );
                }
                if overflow && capacity > 0 {
                    frame.text(
                        FontKind::Mono,
                        size,
                        text.x,
                        y_at(n),
                        "…",
                        self.p.text_muted,
                        clip,
                    );
                }
            }
            JsonExcerpt::Source(lines) => {
                for (i, line) in lines.iter().take(capacity).enumerate() {
                    frame.text(
                        FontKind::Mono,
                        size,
                        text.x,
                        y_at(i),
                        line,
                        self.p.text_muted,
                        clip,
                    );
                }
            }
        }
    }

    fn page(
        &self,
        frame: &mut Frame<'_>,
        rect: Rect,
        title: &str,
        lines: &[PreviewLine],
        show_blank: bool,
    ) {
        let margin = (rect.w * 0.075).clamp(10.0, 22.0);
        let body = rect.inset(margin, margin);
        let clip = self.clip.intersect(body);
        if clip.w <= 0.0 || clip.h <= 0.0 {
            return;
        }
        let title_size = (rect.w * 0.074).clamp(12.0, 20.0);
        let title_lines = crate::layout::wrap_text(title, body.w, |s| {
            frame.measure_faux(FontKind::Ui, BOLD, title_size, s)
        });
        let mut y = body.y + title_size;
        for title in title_lines.iter().take(2) {
            frame.text_faux(
                FontKind::Ui,
                BOLD,
                title_size,
                body.x,
                y,
                title,
                self.p.text,
                clip,
            );
            y += title_size * 1.18;
        }
        if lines.is_empty() && show_blank {
            let size = title_size * 0.62;
            frame.text(
                FontKind::Ui,
                size,
                body.x,
                y,
                t!("workspace-library-blank"),
                self.p.text_muted,
                clip,
            );
        }
        let normal = (rect.w * 0.04).clamp(7.0, 10.8);
        for (i, line) in lines.iter().enumerate() {
            if y > clip.bottom() {
                break;
            }
            let (text, size, kind, faux, indent) = match line {
                PreviewLine::Text(s) => (s, normal, FontKind::Ui, Faux::default(), 0.0),
                PreviewLine::Heading(s) if i == 0 && s == title => continue,
                PreviewLine::Heading(s) => (s, normal * 1.3, FontKind::Ui, BOLD, 0.0),
                PreviewLine::List(s) => {
                    frame.text(FontKind::Ui, normal, body.x, y, "•", self.p.text, clip);
                    (s, normal, FontKind::Ui, Faux::default(), normal * 1.4)
                }
                PreviewLine::Quote(s) => (s, normal, FontKind::Ui, Faux::default(), normal * 1.2),
                PreviewLine::Code(s) => (
                    s,
                    normal * 0.94,
                    FontKind::Mono,
                    Faux::default(),
                    normal * 0.6,
                ),
                PreviewLine::Rule => {
                    frame.rect_clipped(
                        Rect::new(body.x, y - normal * 0.4, body.w, 0.75),
                        self.p.border,
                        0.0,
                        clip,
                    );
                    y += normal * 1.2;
                    continue;
                }
            };
            let wrapped = crate::layout::wrap_text(text, (body.w - indent).max(1.0), |s| {
                frame.measure_faux(kind, faux, size, s)
            });
            let line_h = size * 1.4;
            for s in wrapped {
                if y > clip.bottom() {
                    break;
                }
                if matches!(line, PreviewLine::Code(_)) {
                    frame.rect_clipped(
                        Rect::new(body.x, y - size, body.w, line_h),
                        self.p.hover,
                        0.0,
                        clip,
                    );
                }
                if matches!(line, PreviewLine::Quote(_)) {
                    frame.rect_clipped(
                        Rect::new(body.x, y - size, 1.5, line_h),
                        self.p.border,
                        0.0,
                        clip,
                    );
                }
                frame.text_faux(
                    kind,
                    faux,
                    size,
                    body.x + indent,
                    y,
                    s,
                    if faux.bold {
                        self.p.text
                    } else {
                        self.p.text_muted
                    },
                    clip,
                );
                y += line_h;
            }
            y += normal * 0.48;
        }
    }

    /// 文件夹后板、逐张展开的叠纸、折下的前袋和底部名称。
    fn folder(
        &self,
        frame: &mut Frame<'_>,
        rect: Rect,
        title: &str,
        info: Option<&FolderPreview>,
        pose: FolderPose,
        library: &Library,
    ) {
        // 背板和前袋轮廓来自 Quiet MD home_folder.rs；MIT 署名见 NOTICE。
        let base = Rect::new(rect.x, rect.y + pose.lift(), rect.w, rect.h);
        let back = folder_outline(
            base,
            0.0,
            0.06 * rect.h,
            0.0,
            0.4 * rect.w,
            [-22.0, -6.0, -2.0, 14.0],
        );
        let ink = if self.p.dark {
            Color::hex(0xffffff)
        } else {
            Color::hex(0)
        };
        frame.polygon_gradient(
            &back,
            (base.y, ink.alpha(0.11)),
            (base.bottom(), ink.alpha(0.07)),
            self.clip,
        );

        if let Some(info) = info
            && let Some(card) = &info.front
        {
            for i in (0..info.papers).rev() {
                let (dx, top, angle) = pose.paper(i, rect.w, rect.h);
                let sheet = Rect::new(
                    rect.x + rect.w * 0.10 + dx,
                    rect.y + top,
                    rect.w * 0.8,
                    rect.h * 0.64,
                );
                let note = matches!(card.content, CardContent::Note(_));
                self.tilted_sheet(frame, sheet, angle, info.seeds[i], i != 0 && note);
                if i == 0 {
                    if let CardContent::Note(lines) = &card.content {
                        self.page(frame, sheet, &card.name, lines, false);
                    } else {
                        self.attachment(
                            frame,
                            sheet,
                            &card.name,
                            library.attachment_preview(&card.path),
                            true,
                        );
                    }
                }
            }
        }

        let lean = rect.h * (0.03 * pose.flap + 0.05 * pose.dip);
        let left = 0.49 * rect.h + lean;
        let right = 0.54 * rect.h + lean;
        let front = folder_outline(
            base,
            left,
            right,
            -3.0 * pose.flap,
            0.44 * rect.w,
            [-16.0, 6.0, 6.0, 26.0],
        );
        let lift = -pose.lift() / 4.0;
        let shadow_y = base.y + right + 10.0;
        let shadow = Rect::new(
            base.x + 6.0,
            shadow_y,
            (base.w - 12.0).max(0.0),
            (base.bottom() - shadow_y).max(0.0),
        );
        if shadow.w > 0.0 && shadow.h > 0.0 {
            frame.within(self.shadow_clip, |frame| {
                frame.shadow(
                    shadow,
                    18.0,
                    22.0 + 10.0 * lift,
                    8.0 + 6.0 * lift,
                    Color::hex(0).alpha(if self.p.dark {
                        0.35
                    } else {
                        0.06 + 0.04 * lift
                    }),
                    None,
                )
            });
        }
        let top = base.y + left;
        frame.polygon_gradient(
            &front,
            (top, self.p.popup_bg.alpha(0.78)),
            (
                top + (base.bottom() - top) * 0.42,
                self.p.popup_bg.alpha(0.97),
            ),
            self.clip,
        );
        frame.polygon_gradient(
            &front,
            (top, ink.alpha(0.012)),
            (base.bottom(), ink.alpha(0.035)),
            self.clip,
        );
        let line = |(a, b): (&(f32, f32), &(f32, f32))| Stroke::Line(a.0, a.1, b.0, b.1);
        let hair: Vec<_> = front
            .iter()
            .zip(front.iter().cycle().skip(1))
            .map(line)
            .collect();
        frame.stroke(&hair, 1.0, ink.alpha(0.07), self.clip);
        frame.stroke(
            &hair[..26],
            1.5,
            Color::hex(0xffffff).alpha(if self.p.dark {
                0.18
            } else {
                0.85 + 0.15 * pose.flap.clamp(0.0, 1.0)
            }),
            self.clip,
        );

        let pad = (rect.w * 0.09).min(16.0);
        let size = 15.0;
        let tools = if rect.w > 160.0 { 72.0 } else { 0.0 };
        let title = ellipsize(frame, title, size, (rect.w - 2.0 * pad - tools).max(0.0));
        let label = info.map_or_else(
            || t!("workspace-library-folder").into(),
            |i| t!("workspace-library-count", count = i.count),
        );
        frame.text_faux(
            FontKind::Ui,
            BOLD,
            size,
            rect.x + pad,
            base.bottom() - 14.0 - 18.0 - 3.0,
            &title,
            self.p.text,
            self.clip,
        );
        frame.text(
            FontKind::Ui,
            12.0,
            rect.x + pad,
            base.bottom() - 14.0 - 3.0,
            &label,
            self.p.text_muted,
            self.clip,
        );
    }

    fn folder_button(
        &self,
        frame: &mut Frame<'_>,
        rect: Rect,
        target: LibraryTarget,
        hover: Hover,
        reveal: f32,
    ) {
        frame.within(self.clip, |frame| {
            frame.with_opacity(reveal, |frame| {
                frame.shadow(
                    rect,
                    14.0,
                    7.0,
                    2.0,
                    Color::hex(0).alpha(0.15),
                    Some(frame.covered(rect, 14.0)),
                );
                let fill = if hover == Hover::Library(target) {
                    over(self.p.popup_bg, self.p.hover)
                } else {
                    self.p.popup_bg
                };
                frame.rect_clipped(rect, fill, 14.0, self.clip);
                let at = center(rect);
                match target {
                    LibraryTarget::NewInFolder(_) => icons::plus(
                        frame,
                        at,
                        15.0 * (0.7 + 0.3 * reveal),
                        self.p.text,
                        self.clip,
                    ),
                    LibraryTarget::FolderMenu(_) => icons::more(frame, at, self.p.text, self.clip),
                    _ => {}
                }
            })
        });
    }

    fn tilted_sheet(&self, frame: &mut Frame<'_>, rect: Rect, angle: f32, seed: u64, sketch: bool) {
        let (cx, cy) = center(rect);
        let (s, c) = angle.sin_cos();
        let turn = |(x, y)| {
            let (dx, dy) = (x - cx, y - cy);
            (cx + c * dx - s * dy, cy + s * dx + c * dy)
        };
        frame.within(self.clip, |frame| {
            frame.shadow(
                rect,
                10.0,
                10.0,
                3.0,
                Color::hex(0).alpha(if self.p.dark { 0.30 } else { 0.07 }),
                Some(frame.covered(rect.inset(3.0, 3.0), 9.0)),
            )
        });
        rounded_surface(
            frame,
            rect.inset(-0.5, -0.5),
            10.5,
            self.p.text.alpha(0.08),
            self.clip,
            turn,
        );
        rounded_surface(frame, rect, 10.0, self.p.popup_bg, self.clip, turn);
        if !sketch || rect.w <= 24.0 {
            return;
        }
        // Quiet MD's seed-based sketches for the papers behind the real preview.
        let mut rng = seed.max(1);
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % 1000) as f32 / 1000.0
        };
        let (x, full) = (rect.x + 12.0, rect.w - 24.0);
        let len = full * (0.42 + 0.3 * next());
        let title = Rect::new(x, rect.y + 14.0, len, 4.0);
        rounded_surface(frame, title, 2.0, self.p.text.alpha(0.55), self.clip, turn);
        let mut y = rect.y + 30.0;
        while y < rect.bottom() - 14.0 {
            let len = if next() < 0.2 {
                full * (0.3 + 0.3 * next())
            } else {
                full * (0.72 + 0.28 * next())
            };
            rounded_surface(
                frame,
                Rect::new(x, y - 1.5, len, 3.0),
                1.5,
                self.p.text.alpha(0.10),
                self.clip,
                turn,
            );
            y += 8.5;
        }
    }
}

/// 上沿的台阶沿 Quiet MD 的曲线；每条水平扫描线只覆盖一个区间。
fn folder_outline(
    rect: Rect,
    left: f32,
    right: f32,
    inset: f32,
    step: f32,
    controls: [f32; 4],
) -> Vec<(f32, f32)> {
    let w = rect.w;
    let h = rect.h;
    let r = 18.0_f32.min(w * 0.12).min(h * 0.12);
    let unit = (w / 190.0).min(1.0);
    let [from, c1, c2, to] = controls.map(|v| step + v * unit);
    let mut points = Vec::with_capacity(45);
    let mut curve = |[a, b, c, d]: [(f32, f32); 4]| {
        points.extend((0..=8).map(|i| {
            let t = i as f32 / 8.0;
            let s = 1.0 - t;
            let mix = |a, b, c, d| {
                s * s * s * a + 3.0 * s * s * t * b + 3.0 * s * t * t * c + t * t * t * d
            };
            (
                rect.x + mix(a.0, b.0, c.0, d.0),
                rect.y + mix(a.1, b.1, c.1, d.1),
            )
        }));
    };
    let quad = |start: (f32, f32), ctrl: (f32, f32), end: (f32, f32)| {
        let control = |(x, y): (f32, f32)| ((x + 2.0 * ctrl.0) / 3.0, (y + 2.0 * ctrl.1) / 3.0);
        [start, control(start), control(end), end]
    };
    curve(quad((inset, left + r), (inset, left), (inset + r, left)));
    curve([(from, left), (c1, left), (c2, right), (to, right)]);
    curve(quad(
        (w - inset - r, right),
        (w - inset, right),
        (w - inset, right + r),
    ));
    curve(quad((w, h - r), (w, h), (w - r, h)));
    curve(quad((r, h), (0.0, h), (0.0, h - r)));
    points
}

/// 用变形后的凸圆角轮廓绘制纸张，不变更卡片的点击区域。
fn rounded_surface(
    frame: &mut Frame<'_>,
    rect: Rect,
    radius: f32,
    color: Color,
    clip: Rect,
    map: impl Fn((f32, f32)) -> (f32, f32),
) {
    use std::f32::consts::FRAC_PI_2;
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let r = radius.min(rect.w.min(rect.h) * 0.5);
    let centers = [
        (rect.right() - r, rect.y + r),
        (rect.right() - r, rect.bottom() - r),
        (rect.x + r, rect.bottom() - r),
        (rect.x + r, rect.y + r),
    ];
    let outline: [(f32, f32); 24] = std::array::from_fn(|i| {
        let (x, y) = centers[i / 6];
        let angle = ((i / 6) as f32 - 1.0 + (i % 6) as f32 / 5.0) * FRAC_PI_2;
        let (s, c) = angle.sin_cos();
        map((x + r * c, y + r * s))
    });
    frame.polygon(&outline, color, clip);
}

pub(super) fn home_icon(frame: &mut Frame<'_>, (x, y): (f32, f32), color: Color, clip: Rect) {
    use Stroke::Line;
    frame.stroke(
        &[
            Line(x - 7.0, y - 1.0, x, y - 7.0),
            Line(x, y - 7.0, x + 7.0, y - 1.0),
            Line(x - 5.5, y - 2.0, x - 5.5, y + 6.0),
            Line(x + 5.5, y - 2.0, x + 5.5, y + 6.0),
            Line(x - 5.5, y + 6.0, x + 5.5, y + 6.0),
        ],
        1.4,
        color,
        clip,
    );
}
