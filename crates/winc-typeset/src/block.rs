//! 各类块的排版：标题、引用、独占一行的公式和可折叠的块。

use super::*;

impl Cx<'_, '_> {
    /// 排一个 `level` 级标题；一、二级标题下方加一道横线，并在 `out` 中记下标题的锚点。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn heading(
        &mut self,
        level: u8,
        inline: &Inline,
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        let base = self.base;
        let scale = [2.0, 1.5, 1.25, 1.0, 0.875, 0.85][(level as usize).clamp(1, 6) - 1];
        let size = base * scale;
        let color = if level == 6 {
            self.colors.muted
        } else {
            env.color
        };
        let para = Para {
            size,
            bold: true,
            color,
            line: 1.25,
            align: Align::None,
        };
        out.anchors.push((slug(&inline.text), y));
        let mut flow = self.flow(inline, x, y, width, para, &mut out.items);
        if level <= 2 {
            let line_y = y + flow.height + size * 0.3;
            out.items.push(Item::Fill {
                rect: Rect::new(x, line_y, width, 1.0),
                color: self.colors.border,
                radius: 0.0,
            });
            flow.height += size * 0.3 + 1.0;
        }
        flow
    }

    /// 排一段引用：左侧一道竖线，内容缩进；`alert` 不为 `None` 时是提示块，竖线和标题用其颜色。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn quote(
        &mut self,
        alert: Option<Alert>,
        blocks: &[Block],
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        let base = self.base;
        let para = self.body_para(env);
        let bar = (base * 0.25).round();
        let inner = x + bar + base;
        let room = (width - bar - base).max(1.0);
        let (bar_color, env) = match alert {
            Some(alert) => (Colors::alert(self.dark, alert), env),
            None => (
                self.colors.border,
                Env {
                    color: self.colors.muted,
                    ..env
                },
            ),
        };
        let mut top = y;
        if let Some(alert) = alert {
            let title = match alert {
                Alert::Note => "Note",
                Alert::Tip => "Tip",
                Alert::Important => "Important",
                Alert::Warning => "Warning",
                Alert::Caution => "Caution",
            };
            let inline = Inline {
                text: title.to_owned(),
                runs: vec![winc_preview::markdown::Run {
                    end: title.len(),
                    style: Style::default(),
                }],
                ..Inline::default()
            };
            let para = Para {
                bold: true,
                color: bar_color,
                ..para
            };
            let flow = self.flow(&inline, inner, top, room, para, &mut out.items);
            top += flow.height + base * 0.25;
        }
        let flow = self.blocks(blocks, inner, top, room, env, out);
        let height = top + flow.height - y;
        out.items.push(Item::Fill {
            rect: Rect::new(x, y, bar, height),
            color: bar_color,
            radius: 0.0,
        });
        Flow {
            height,
            first_baseline: flow.first_baseline,
        }
    }

    /// 排一个独占一行的公式，水平居中；`source` 是公式在原文中的位置，点击公式时光标移到那里。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn math_block(
        &mut self,
        tex: &str,
        source: usize,
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        let base = self.base;
        let mut b = self.math(tex, base, true, env.color);
        // 比一行宽的缩小（最多缩到一半），还宽就让它超出去。
        if b.w > width {
            let size = base * (width / b.w).max(0.5);
            b = self.math(tex, size, true, env.color);
        }
        let pad = base * 0.5;
        let (dx, baseline) = (x + ((width - b.w) / 2.0).max(0.0), y + pad + b.asc);
        out.items
            .extend(b.items.iter().map(|item| item.moved(dx, baseline, 0)));
        // 点击公式时光标移到公式的原文处，进入编辑。
        let height = pad * 2.0 + b.asc + b.desc;
        out.items.push(Item::Spot {
            rect: Rect::new(x, y, width, height),
            at: (self.chars, u32::try_from(source).unwrap_or(0)),
        });
        // 复制、查找时是公式的原文。
        self.text.push_str(tex);
        self.chars += tex.chars().count();
        self.end_paragraph();
        Flow {
            height,
            first_baseline: Some(baseline),
        }
    }

    /// 排一个可折叠的块：标题一行前加展开或收起的箭头，展开时在其下排 `blocks`。
    ///
    /// `id` 标识这个块，`open` 是原文中的初始状态；用户点击标题切换过时取相反的状态。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn details(
        &mut self,
        id: u64,
        open: bool,
        summary: &Inline,
        blocks: &[Block],
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        let base = self.base;
        let property_panel = summary
            .links
            .first()
            .is_some_and(|url| url == winc_preview::markdown::properties::PANEL);
        let para = if property_panel {
            Para {
                size: base * 0.86,
                color: self.colors.muted,
                ..self.body_para(env)
            }
        } else {
            self.body_para(env)
        };
        let open = open != self.toggled.contains(&id);
        let indent = (base * 1.25).round();
        let flow = self.flow(summary, x + indent, y, width - indent, para, &mut out.items);
        let arrow = if open { "▾" } else { "▸" };
        self.marker(
            arrow,
            x + indent,
            y,
            flow.first_baseline,
            para.color,
            &mut out.items,
        );
        // 整行都能点；排在标题的链接后面，标题里的链接先认。
        out.items.push(Item::Link {
            rect: Rect::new(x, y, width, flow.height),
            url: format!("{DETAILS}{id:x}").into(),
        });
        if !open {
            return flow;
        }
        let top = y + flow.height + base * 0.5;
        let inner = if property_panel {
            self.property_rows(blocks, x, top, width, env, out)
        } else {
            self.blocks(blocks, x, top, width, env, out)
        };
        Flow {
            height: top + inner.height - y,
            first_baseline: flow.first_baseline,
        }
    }

    fn property_rows(
        &mut self,
        blocks: &[Block],
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        use winc_preview::markdown::properties::{Kind, PROPERTY};
        let mut row_y = y;
        let size = self.base * 0.88;
        let pad = size * 0.30;
        let line = size * 1.5;
        let value_style = Para {
            size,
            ..self.body_para(env)
        };
        let name_style = Para {
            color: self.colors.muted,
            ..value_style
        };
        let label_width = (width * 0.34).min(size * 10.5);
        let label_inset = size * 1.66;
        for block in blocks {
            if let Block::Table(_, rows, _) = block {
                for row in rows {
                    let [label, value] = row.as_slice() else {
                        continue;
                    };
                    let kind = label
                        .links
                        .first()
                        .and_then(|id| Kind::from_id(id))
                        .unwrap_or(Kind::Text);
                    let paint_start = out.items.len();
                    let top = row_y + pad;
                    let cy = top + line * 0.5;
                    let left = x + label_width + pad * 1.5;
                    let room = (x + width - left - pad).max(1.0);
                    let name = self.flow(
                        label,
                        x + label_inset,
                        top,
                        (label_width - label_inset - pad).max(1.0),
                        name_style,
                        &mut out.items,
                    );
                    self.property_icon(kind, (x + size * 0.73, cy), size * 0.80, &mut out.items);
                    let height = if kind.is_choice() && value.links.len() > 1 {
                        self.property_chips(
                            &value.links[1..],
                            left,
                            top,
                            room,
                            value_style,
                            &mut out.items,
                        )
                    } else if kind == Kind::Checkbox && !value.links.is_empty() {
                        let checked = value.text == "☑";
                        let d = size * 0.95;
                        let r = Rect::new(left, cy - d / 2.0, d, d);
                        let color = if checked {
                            self.accent
                        } else {
                            self.colors.muted
                        };
                        out.items.push(Item::Fill {
                            rect: r,
                            color,
                            radius: 2.5,
                        });
                        if checked {
                            let (x, y) = (r.x, r.y);
                            out.items.push(Item::path(
                                vec![
                                    Stroke::Line(
                                        x + d * 0.22,
                                        y + d * 0.51,
                                        x + d * 0.42,
                                        y + d * 0.71,
                                    ),
                                    Stroke::Line(
                                        x + d * 0.42,
                                        y + d * 0.71,
                                        x + d * 0.79,
                                        y + d * 0.30,
                                    ),
                                ],
                                1.4,
                                self.surface,
                            ));
                        } else {
                            out.items.push(Item::Fill {
                                rect: r.inset(1.2, 1.2),
                                color: self.surface,
                                radius: 1.5,
                            });
                        }
                        line
                    } else if value.text.is_empty() {
                        let mut empty = Inline::default();
                        empty.push("—", Style::default());
                        self.flow(&empty, left, top, room, name_style, &mut out.items)
                            .height
                    } else {
                        let style = if kind == Kind::Url {
                            Para {
                                color: self.colors.link,
                                ..value_style
                            }
                        } else {
                            value_style
                        };
                        self.flow(value, left, top, room, style, &mut out.items)
                            .height
                    }
                    .max(name.height)
                    .max(line)
                        + 2.0 * pad;
                    if let Some(target) = value.links.first() {
                        let index = target.strip_prefix(PROPERTY).unwrap_or_default();
                        out.items.splice(
                            paint_start..paint_start,
                            [
                                Item::Link {
                                    rect: Rect::new(x, row_y, label_width, height),
                                    url: format!("{PROPERTY}name:{index}").into(),
                                },
                                Item::Link {
                                    rect: Rect::new(
                                        x + label_width,
                                        row_y,
                                        width - label_width,
                                        height,
                                    ),
                                    url: target.as_str().into(),
                                },
                            ],
                        );
                    }
                    row_y += height + 1.0;
                }
            } else if let Block::Paragraph(add) = block {
                let top = row_y + pad;
                let insert = out.items.len();
                let flow = self.flow(
                    add,
                    x + pad,
                    top,
                    (width - pad).max(1.0),
                    name_style,
                    &mut out.items,
                );
                if let Some(url) = add.links.first() {
                    out.items.insert(
                        insert,
                        Item::Link {
                            rect: Rect::new(
                                x,
                                top - pad,
                                label_width.max(size * 9.0).min(width),
                                flow.height + pad * 2.0,
                            ),
                            url: url.as_str().into(),
                        },
                    );
                }
                row_y += flow.height + pad * 2.0;
            }
        }
        Flow {
            height: row_y - y,
            first_baseline: None,
        }
    }

    fn property_chips(
        &mut self,
        labels: &[String],
        x: f32,
        y: f32,
        width: f32,
        para: Para,
        items: &mut Vec<Item>,
    ) -> f32 {
        let line = para.size * para.line;
        let gap = para.size * 0.36;
        let pad = para.size * 0.42;
        let (mut dx, mut dy, mut used) = (0.0, 0.0, 0.0_f32);
        for label in labels {
            let w = (self.frame.measure(FontKind::Ui, para.size, label) + 2.0 * pad).min(width);
            if dx > 0.0 && dx + w > width {
                dx = 0.0;
                dy += used + gap * 0.6;
                used = 0.0;
            }
            let (fg, bg) = crate::property_chip_colors(label, self.dark);
            let mut inline = Inline::default();
            inline.push(label, Style::default());
            let i = items.len();
            let h = self
                .flow(
                    &inline,
                    x + dx + pad,
                    y + dy,
                    (w - 2.0 * pad).max(1.0),
                    Para { color: fg, ..para },
                    items,
                )
                .height
                .max(line);
            items.insert(
                i,
                Item::Fill {
                    rect: Rect::new(x + dx, y + dy, w, h),
                    color: bg,
                    radius: 4.0,
                },
            );
            used = used.max(h);
            dx += w + gap;
        }
        dy + used
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

    /// 返回正文段落的样式：正文字号、不加粗、颜色取自 `env`。
    pub(super) fn body_para(&self, env: Env) -> Para {
        Para {
            size: self.base,
            bold: false,
            color: env.color,
            line: 1.5,
            align: Align::None,
        }
    }
}
