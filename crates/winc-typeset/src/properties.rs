//! Layout for the front matter panel in the writing view.

use super::*;
use winc_preview::markdown::properties::{ADD, COLLAPSE, FIELDS, Kind, PROPERTY, RAW, SOURCE};

impl Cx<'_, '_> {
    pub(super) fn property_panel(
        &mut self,
        open: bool,
        summary: &Inline,
        blocks: &[Block],
        area: Rect,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        let first = out.items.len();
        let size = self.base * 0.84;
        let inset = (size * 0.65).round();
        let inner_width = (area.w - inset * 2.0).max(0.0);
        let header = Rect::new(
            area.x + inset,
            area.y + 5.0,
            inner_width,
            (size * 2.2).max(28.0),
        );
        let title_w = (self.frame.measure(FontKind::Ui, size, &summary.text) + 32.0).min(header.w);
        let toggle = Rect {
            w: title_w,
            ..header
        };
        link(if open { toggle } else { header }, COLLAPSE, &mut out.items);
        let title = Rect {
            x: toggle.x + 14.0,
            w: (toggle.w - 14.0).max(0.0),
            ..toggle
        };
        self.property_text(&summary.text, title, size, self.colors.text, &mut out.items);
        let disclosure = Rect {
            w: 20.0_f32.min(toggle.w),
            ..toggle
        };
        control_icon(
            disclosure,
            if open {
                Control::Collapse
            } else {
                Control::Expand
            },
            self.colors.muted,
            &mut out.items,
        );

        if open {
            let source = matches!(blocks, [Block::Code { .. }]);
            let segment = (size * 2.2)
                .round()
                .min((header.w - title_w).max(0.0) * 0.5);
            let modes = Rect::new(
                header.right() - 2.0 * segment,
                header.y + 2.0,
                segment * 2.0,
                header.h - 4.0,
            );
            out.items.push(Item::Fill {
                rect: modes,
                color: self.colors.code_inline,
                radius: 6.0,
            });
            for (i, (icon, url, selected)) in [
                (Control::Fields, FIELDS, !source),
                (Control::Source, SOURCE, source),
            ]
            .into_iter()
            .enumerate()
            {
                let slot = Rect {
                    x: modes.x + segment * i as f32,
                    w: segment,
                    ..modes
                };
                if selected {
                    out.items.push(Item::Fill {
                        rect: slot.inset(2.0, 2.0),
                        color: self.surface,
                        radius: 4.0,
                    });
                }
                link(slot, url, &mut out.items);
                control_icon(
                    slot,
                    icon,
                    if selected {
                        self.accent
                    } else {
                        self.colors.muted
                    },
                    &mut out.items,
                );
            }

            if let Some(Block::Paragraph(add)) = blocks.get(1) {
                let right = modes.x - 6.0;
                let room = (right - toggle.right() - 4.0).max(0.0);
                let label_width = self.frame.measure(FontKind::Ui, size, &add.text) + 15.0;
                let icon_width = 22.0;
                let show_label = room >= label_width + icon_width;
                let width = if show_label {
                    icon_width + label_width
                } else {
                    room.min(icon_width)
                };
                let button = Rect::new(right - width, header.y + 2.0, width, header.h - 4.0);
                link(button, ADD, &mut out.items);
                control_icon(
                    Rect {
                        w: button.w.min(icon_width),
                        ..button
                    },
                    Control::Add,
                    self.colors.muted,
                    &mut out.items,
                );
                if show_label {
                    let label = Rect {
                        x: button.x + icon_width - 5.0,
                        w: label_width + 5.0,
                        ..button
                    };
                    self.property_text(&add.text, label, size, self.colors.text, &mut out.items);
                }
            }
        } else if summary.links.len() > 1 {
            let names = summary.links[1..].join(" · ");
            let rect = Rect {
                x: toggle.right() + 3.0,
                w: (header.w - title_w - 3.0).max(0.0),
                ..header
            };
            self.property_text(&names, rect, size, self.colors.muted, &mut out.items);
        }

        let top = header.bottom() + 9.0;
        let inside = Rect::new(header.x, top, header.w, 0.0);
        let height = if open {
            let body_height = match blocks.first() {
                Some(Block::Table(_, rows, _)) => self.property_rows(rows, inside, env, out),
                Some(Block::Code { text, .. }) => self.property_source(text, inside, out),
                _ => 0.0,
            };
            top - area.y + body_height + inset
        } else {
            header.bottom() - area.y + 5.0
        };
        out.items.insert(
            first,
            Item::Fill {
                rect: Rect::new(area.x, area.y, area.w, height),
                color: self
                    .colors
                    .text
                    .alpha(if self.dark { 0.045 } else { 0.026 }),
                radius: 9.0,
            },
        );
        Flow {
            height,
            first_baseline: None,
        }
    }

    fn property_source(&mut self, text: &str, area: Rect, out: &mut Out) -> f32 {
        let size = self.base * 0.83;
        let line = 20.0;
        let metrics = self.frame.metrics(FontKind::Mono, size);
        let row = Row {
            room: (area.w - 20.0).max(size),
            size,
            line_h: line,
            ascent: metrics.ascent,
            descent: metrics.descent,
        };
        let max_height = line * 14.0;
        let mut top = 5.0;
        let mut state = winc_syntax::LineState::default();
        for line in text.split('\n') {
            let piece = self.code_piece(line, 0, "", None, &mut state, row);
            for item in &piece.items {
                if let Item::Text { baseline, .. } = item
                    && top + baseline < max_height + 5.0
                {
                    let mut item = item.moved(area.x + 10.0, area.y + top, 0);
                    if let Item::Text { span, .. } = &mut item {
                        *span = None;
                    }
                    out.items.push(item);
                }
            }
            top += piece.height;
            if top >= max_height + 5.0 {
                break;
            }
        }
        let height = top.clamp(row.line_h * 3.0, max_height + 5.0) + 5.0;
        link(
            Rect::new(area.x, area.y, area.w, height),
            RAW,
            &mut out.items,
        );
        height
    }

    fn property_rows(&mut self, rows: &[Vec<Inline>], area: Rect, env: Env, out: &mut Out) -> f32 {
        let size = self.base * 0.83;
        let height = (size * 2.08).round().max(27.0);
        let label_width = (area.w * 0.29).min(size * 12.0);
        let mut y = area.y;
        for row in rows {
            let [label, value] = row.as_slice() else {
                continue;
            };
            let kind = label
                .links
                .first()
                .and_then(|id| Kind::from_id(id))
                .unwrap_or(Kind::Text);
            let (x, width) = (area.x, area.w);
            let icon = Rect::new(x, y, (size * 1.9).min(label_width), height);
            let name = Rect::new(icon.right(), y, (label_width - icon.w).max(0.0), height);
            let delete = Rect::new(area.right() - height, y, height, height);
            let cell = Rect::new(name.right(), y, (delete.x - name.right()).max(0.0), height);
            let start = out.items.len();
            let mut row_height = height;
            let index = value
                .links
                .first()
                .and_then(|target| target.strip_prefix(PROPERTY));
            if let Some(index) = index {
                link(name, format!("{PROPERTY}name:{index}"), &mut out.items);
                link(icon, format!("{PROPERTY}type:{index}"), &mut out.items);
            }
            self.property_icon(
                kind,
                (x + icon.w * 0.52, y + height * 0.5),
                size * 0.85,
                &mut out.items,
            );
            self.property_text(&label.text, name, size, self.colors.muted, &mut out.items);
            if let Some(index) = index {
                if matches!(kind, Kind::List | Kind::MultiSelect) {
                    row_height =
                        self.property_list(&value.links[1..], index, cell, size, &mut out.items);
                } else if kind == Kind::Checkbox {
                    self.property_switch(cell, value.text == "☑", size, &mut out.items);
                } else if kind == Kind::Select
                    && let Some(label) = value.links.get(1)
                {
                    let (fg, bg) = crate::property_chip_colors(label, self.dark);
                    let w = (self.frame.measure(FontKind::Ui, size, label) + 12.0).min(cell.w);
                    let chip_h = size + 6.0;
                    let chip = Rect::new(cell.x + 5.0, cell.y + (cell.h - chip_h) * 0.5, w, chip_h);
                    out.items.push(Item::Fill {
                        rect: chip,
                        color: bg,
                        radius: 5.0,
                    });
                    self.property_text(label, chip, size, fg, &mut out.items);
                } else {
                    let text = value.text.replace(['\r', '\n'], " ");
                    let (text, color) = if text.is_empty() {
                        ("—", self.colors.muted.alpha(0.65))
                    } else {
                        (text.as_str(), env.color)
                    };
                    self.property_text(text, cell, size, color, &mut out.items);
                }
                link(
                    Rect {
                        h: row_height,
                        ..cell
                    },
                    value.links[0].as_str(),
                    &mut out.items,
                );
                link(delete, format!("{PROPERTY}delete:{index}"), &mut out.items);
                out.items.insert(
                    start,
                    Item::Link {
                        rect: Rect::new(x, y, width, row_height),
                        url: format!("{PROPERTY}row:{index}").into(),
                    },
                );
            } else {
                self.property_text(&value.text, cell, size, self.colors.muted, &mut out.items);
            }
            y += row_height + 2.0;
        }
        (y - area.y - 2.0).max(0.0)
    }

    fn property_list(
        &mut self,
        labels: &[String],
        index: &str,
        cell: Rect,
        size: f32,
        items: &mut Vec<Item>,
    ) -> f32 {
        let gap = 4.0;
        let chip_h = size + 6.0;
        let inset = 5.0;
        let close = (size + 3.0).max(16.0);
        let mut x = cell.x + inset;
        let mut y = cell.y + (cell.h - chip_h) * 0.5;
        let right = cell.right() - inset;
        for (i, label) in labels.iter().enumerate() {
            let w = (self.frame.measure(FontKind::Ui, size, label) + 2.0 * inset + close)
                .min((right - cell.x - inset).max(0.0));
            if x > cell.x + inset && x + w > right {
                x = cell.x + inset;
                y += cell.h;
            }
            let chip = Rect::new(x, y, w, chip_h);
            let (fg, bg) = crate::property_chip_colors(label, self.dark);
            items.push(Item::Fill {
                rect: chip,
                color: bg,
                radius: 5.0,
            });
            self.property_text(
                label,
                Rect {
                    w: (w - close + 1.0).max(0.0),
                    ..chip
                },
                size,
                fg,
                items,
            );
            let remove_w = (close + 3.0).min(w);
            let remove = Rect::new(chip.right() - remove_w, y, remove_w, chip_h);
            link(remove, format!("{PROPERTY}item:{index}:{i}"), items);
            if remove.w >= close {
                let (x, y) = (remove.x + remove.w / 2.0, remove.y + remove.h / 2.0);
                let d = size * 0.20;
                items.push(Item::path(
                    vec![
                        Stroke::Line(x - d, y - d, x + d, y + d),
                        Stroke::Line(x + d, y - d, x - d, y + d),
                    ],
                    1.0,
                    fg,
                ));
            }
            x += w + gap;
        }
        // Keep space to enter an item even after a long chip at the edge of the column.
        let input_width = (cell.w * 0.42).min(size * 8.0);
        if x > cell.x + inset && x + input_width > right {
            x = cell.x + inset;
            y += cell.h;
        }
        let add = Rect::new(x, y, (right - x).clamp(0.0, chip_h), chip_h);
        let hit = Rect::new(add.x, add.y - (cell.h - chip_h) * 0.5, add.w, cell.h);
        link(hit, format!("{PROPERTY}list:{index}"), items);
        control_icon(add, Control::Add, self.colors.muted, items);
        y - cell.y + chip_h + (cell.h - chip_h) * 0.5
    }

    fn property_switch(&self, cell: Rect, checked: bool, size: f32, items: &mut Vec<Item>) {
        let h = (size * 1.07).round();
        let rect = Rect::new(cell.x + 5.0, cell.y + (cell.h - h) * 0.5, h * 1.8, h);
        items.push(Item::Fill {
            rect,
            color: if checked {
                self.accent
            } else {
                self.colors.muted.alpha(0.38)
            },
            radius: h * 0.5,
        });
        let d = h - 4.0;
        items.push(Item::Fill {
            rect: Rect::new(
                if checked {
                    rect.right() - d - 2.0
                } else {
                    rect.x + 2.0
                },
                rect.y + 2.0,
                d,
                d,
            ),
            color: Color::hex(0xffffff),
            radius: d * 0.5,
        });
    }

    fn property_text(
        &mut self,
        text: &str,
        rect: Rect,
        size: f32,
        color: Color,
        items: &mut Vec<Item>,
    ) {
        let room = rect.w - 9.0;
        if room <= 0.0 {
            return;
        }
        let font = FontKind::Ui;
        let text: Arc<str> = if self.frame.measure(font, size, text) <= room {
            text.into()
        } else {
            let mut used = self.frame.measure(font, size, "…");
            let mut end = 0;
            for (i, ch) in text.char_indices() {
                used += self.frame.measure(font, size, ch.encode_utf8(&mut [0; 4]));
                if used > room {
                    break;
                }
                end = i + ch.len_utf8();
            }
            format!("{}…", &text[..end]).into()
        };
        let metrics = self.frame.metrics(font, size);
        items.push(Item::Text {
            x: rect.x + 5.0,
            baseline: rect.y + (rect.h + metrics.ascent - metrics.descent) * 0.5,
            text,
            font,
            faux: Faux::default(),
            size,
            color,
            span: None,
        });
    }

    fn property_icon(
        &mut self,
        kind: winc_preview::markdown::properties::Kind,
        (x, y): (f32, f32),
        size: f32,
        items: &mut Vec<Item>,
    ) {
        use winc_preview::markdown::properties::Kind;
        let color = self.colors.muted;
        let s = size / 2.0;
        let line = |ax: f32, ay: f32, bx: f32, by: f32| {
            Stroke::Line(x + ax * s, y + ay * s, x + bx * s, y + by * s)
        };
        let square = |top| {
            vec![
                line(-0.86, top, 0.86, top),
                line(0.86, top, 0.86, 0.83),
                line(0.86, 0.83, -0.86, 0.83),
                line(-0.86, 0.83, -0.86, top),
            ]
        };
        let strokes = match kind {
            Kind::Date => [
                square(-0.64),
                vec![
                    line(-0.86, -0.17, 0.86, -0.17),
                    line(-0.43, -0.91, -0.43, -0.43),
                    line(0.43, -0.91, 0.43, -0.43),
                ],
            ]
            .concat(),
            Kind::Select => vec![
                Stroke::Circle(x, y, s * 0.89),
                line(-0.36, -0.12, 0.0, 0.26),
                line(0.0, 0.26, 0.36, -0.12),
            ],
            Kind::Checkbox => [
                square(-0.86),
                vec![
                    line(-0.48, -0.01, -0.12, 0.36),
                    line(-0.12, 0.36, 0.51, -0.39),
                ],
            ]
            .concat(),
            Kind::List | Kind::MultiSelect => [-0.60, 0.0, 0.60]
                .into_iter()
                .flat_map(|at| {
                    [
                        Stroke::Circle(x - 0.75 * s, y + at * s, 0.10 * s),
                        line(-0.33, at, 0.82, at),
                    ]
                })
                .collect(),
            Kind::Url => vec![
                line(-0.74, 0.74, 0.66, -0.66),
                line(-0.15, -0.66, 0.66, -0.66),
                line(0.66, -0.66, 0.66, 0.15),
            ],
            Kind::Text => vec![line(-0.80, -0.58, 0.80, -0.58), line(0.0, -0.58, 0.0, 0.78)],
            Kind::Number => vec![
                line(-0.24, -0.80, -0.50, 0.80),
                line(0.50, -0.80, 0.24, 0.80),
                line(-0.76, -0.25, 0.84, -0.25),
                line(-0.84, 0.25, 0.76, 0.25),
            ],
        };
        items.push(Item::path(strokes, 1.15, color));
    }
}
#[derive(Clone, Copy)]
enum Control {
    Collapse,
    Expand,
    Fields,
    Source,
    Add,
}

fn control_icon(rect: Rect, icon: Control, color: Color, items: &mut Vec<Item>) {
    if rect.w < 10.0 || rect.h < 10.0 {
        return;
    }
    let x = (rect.x + rect.w * 0.5).round();
    let y = (rect.y + rect.h * 0.5).round();
    let line = |a, b, c, d| Stroke::Line(x + a, y + b, x + c, y + d);
    let strokes = match icon {
        Control::Collapse => vec![line(-3.5, -1.5, 0.0, 2.0), line(0.0, 2.0, 3.5, -1.5)],
        Control::Expand => vec![line(-1.5, -3.5, 2.0, 0.0), line(2.0, 0.0, -1.5, 3.5)],
        Control::Add => vec![line(-4.0, 0.0, 4.0, 0.0), line(0.0, -4.0, 0.0, 4.0)],
        Control::Fields => [-4.0, 0.0, 4.0]
            .into_iter()
            .flat_map(|dy| {
                [
                    Stroke::Circle(x - 5.0, y + dy, 0.65),
                    line(-2.0, dy, 5.0, dy),
                ]
            })
            .collect(),
        Control::Source => [-1.0, 1.0]
            .into_iter()
            .flat_map(|side| {
                let outer = 6.0 * side;
                let inner = 4.0 * side;
                [
                    line(3.0 * side, -5.0, inner, -5.0),
                    line(inner, -5.0, inner, -2.0),
                    line(inner, -2.0, outer, 0.0),
                    line(outer, 0.0, inner, 2.0),
                    line(inner, 2.0, inner, 5.0),
                    line(inner, 5.0, 3.0 * side, 5.0),
                ]
            })
            .collect(),
    };
    items.push(Item::path(strokes, 1.25, color));
}

fn link(rect: Rect, url: impl Into<Arc<str>>, items: &mut Vec<Item>) {
    if rect.w > 0.0 && rect.h > 0.0 {
        items.push(Item::Link {
            rect,
            url: url.into(),
        });
    }
}
