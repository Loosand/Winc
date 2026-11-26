//! 原样显示的块：实时编辑时光标所在的块按源文字排版，可以带预览、分隔线或边框。

use super::*;

/// 要显示原文的叶子块（可编辑的预览中光标所在的块）。
#[derive(Clone, Hash)]
pub struct Raw {
    /// 所在顶层块现在的原文（从块的起点起，含解析之后的修改）。
    pub block: Arc<str>,
    /// 叶子块在 `block` 中的范围。
    pub leaf: std::ops::Range<usize>,
    /// 叶子块在解析结果中的终点（相对顶层块）。
    ///
    /// 块里记下的原文位置来自解析结果；输入之后、重新解析之前，`leaf` 的终点已按新增的字节
    /// 后移，其后的块仍是旧的位置。判断块是否属于该叶子块时与这个终点比较，其后的块不会落入
    /// 叶子块而被跳过。
    pub parsed_end: usize,
    /// 已经显示过：同一个叶子块里排出的其余块（HTML 块里的几段、表格的其余单元格）不再显示。
    pub done: bool,
    /// 光标在 `block` 中的位置：表格只把光标所在的单元格显示为原文。
    pub caret: usize,
    /// 叶子块的种类，决定原文的显示方式。
    pub kind: RawKind,
    /// 输入法正在组合的文字（仅限正文），排入光标所在的行，见 `reveal`。
    pub preedit: Option<Arc<str>>,
}

/// 显示原文的叶子块的种类。
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum RawKind {
    /// 段落、标题：按排版显示，只显示光标接触到的标记，见 `reveal`。
    Prose,
    /// 围栏代码块、公式（`$$`）：放入代码框按语言着色，围栏行只在光标位于其上时显示；
    /// 公式和 mermaid 图的下方另外绘制渲染结果。
    Fenced(Box<str>),
    /// HTML 块：放入代码框按 HTML 着色，下方另外绘制渲染结果。
    Html,
    /// 其余块：使用等宽字体，按 Markdown 着色。
    Plain,
}

impl Raw {
    /// 光标位于 `block` 的 `caret` 处，所在的叶子块为 `leaf`，在解析结果中的终点为
    /// `parsed_end`；`prose` 表示正文。
    pub fn new(
        block: Arc<str>,
        leaf: std::ops::Range<usize>,
        parsed_end: usize,
        caret: usize,
        prose: bool,
        preedit: Option<Arc<str>>,
    ) -> Raw {
        let mut raw = Raw {
            block,
            parsed_end: parsed_end.max(leaf.start),
            leaf,
            done: false,
            caret,
            kind: RawKind::Plain,
            preedit,
        };
        raw.kind = if prose { RawKind::Prose } else { raw.guess() };
        raw
    }

    /// 按叶子块原文的第一行识别其种类（仅用于非正文的块）。
    pub fn guess(&self) -> RawKind {
        let text = self.block.get(self.leaf.clone()).unwrap_or_default();
        let first = text.split('\n').next().unwrap_or_default();
        let line = first.trim_start_matches(' ');
        if first.len() - line.len() > 3 {
            return RawKind::Plain;
        }
        if let Some(info) = ["```", "~~~"].iter().find_map(|f| line.strip_prefix(f)) {
            let info = info.trim_start_matches(['`', '~']).trim();
            let lang = info.split_whitespace().next().unwrap_or_default();
            return RawKind::Fenced(lang.into());
        }
        if line.starts_with("$$") {
            return RawKind::Fenced("latex".into());
        }
        if line.starts_with('<') {
            return RawKind::Html;
        }
        RawKind::Plain
    }
}

/// 可编辑的预览中光标所在的单元格：第几行（解析出的行，不含表头下面那一行）、第几列，和这一格
/// 两个 `|` 之间的原文在顶层块中的范围。
pub(super) type LiveCell = (usize, usize, std::ops::Range<usize>);

/// [`Cx::raw_state`] 的结果：块不在光标所在的叶子块里、显示原文、或者原文已经显示过。
pub(super) enum RawState {
    Laid,
    Show,
    Skip,
    /// 光标所在的段落、标题：按排版显示，只显示光标接触到的标记。
    ///
    /// 规则见 `winc_preview::markdown::reveal`。当编辑期间拆出多个块时逐块保持排版；
    /// 光标位于段落之后的空行上时，这些空行（原文起点，相对顶层块）各排一行，供光标定位。
    Reveal(Vec<Block>, Vec<usize>),
    /// 光标所在的分隔线，见 [`Cx::raw_rule`]。
    Rule,
}

/// 返回光标位于叶子块内容之后的空行上时需要排版的空行（原文起点，相对顶层块）。
///
/// 范围从内容之后的第一行到光标所在的行；在块末尾按回车后，光标位于这些行上。
pub(super) fn blanks(raw: &Raw) -> Vec<usize> {
    let text = raw.block.get(raw.leaf.clone()).unwrap_or_default();
    let caret = raw.caret.saturating_sub(raw.leaf.start).min(text.len());
    let content = text.trim_end().len();
    match text[content..].find('\n').map(|n| content + n + 1) {
        Some(first) if caret >= first => std::iter::once(first)
            .chain(
                text[first..caret]
                    .match_indices('\n')
                    .map(|(n, _)| first + n + 1),
            )
            .map(|at| raw.leaf.start + at)
            .collect(),
        _ => Vec::new(),
    }
}

/// 返回最后一个非空白行的行尾（不含 `\r`/`\n`，保留该行末尾的空格与制表符）；
/// 若全是水平空白则保留到第一个换行符之前，使输入行尾空格时仍可正常排版并推动光标。
fn content_end(text: &str) -> usize {
    let trimmed = text.trim_end().len();
    let end = match text[trimmed..].find('\n') {
        Some(rel) => trimmed + rel,
        None => text.len(),
    };
    text[..end].trim_end_matches('\r').len()
}

/// 返回光标所在的段落、标题按当前原文生成的块列表（见 `winc_preview::markdown::reveal`）、
/// 光标位于其后的空行上时需要排版的空行（原文起点，相对顶层块），以及组字是否已排入该块。
pub(super) fn reveal(raw: &Raw) -> (Vec<Block>, Vec<usize>, bool) {
    let before = &raw.block[..raw.leaf.start];
    let prefix = before
        .rsplit(['\n', '\r'])
        .next()
        .unwrap_or_default()
        .trim_start_matches([' ', '\t']);
    let before_quote = prefix.starts_with('>')
        && prefix.bytes().all(|c| matches!(c, b'>' | b' ' | b'\t'))
        && !raw
            .block
            .get(raw.leaf.clone())
            .unwrap_or_default()
            .trim()
            .is_empty();
    let start = raw.leaf.start - if before_quote { prefix.len() } else { 0 };
    let text = raw.block.get(start..raw.leaf.end).unwrap_or_default();
    let quote_body = text.trim_start_matches([' ', '\t', '>']);
    // A callout opener displayed at its own source position must keep its line.
    let alert = quote_body.lines().next().is_some_and(|line| {
        [
            "[!NOTE]",
            "[!TIP]",
            "[!IMPORTANT]",
            "[!WARNING]",
            "[!CAUTION]",
        ]
        .iter()
        .any(|kind| line.trim_end().eq_ignore_ascii_case(kind))
    });
    let leading_quote = text.trim_start_matches([' ', '\t']).starts_with('>')
        && !quote_body.trim().is_empty()
        && !alert;
    let caret = raw.caret.saturating_sub(start).min(text.len());
    let content = content_end(text);
    let blanks = blanks(raw);
    let at = (caret <= content).then_some(caret);
    let preedit = raw.preedit.as_deref().filter(|_| at.is_some());
    let mut blocks =
        winc_preview::markdown::reveal::reveal_blocks(&text[..content], at, preedit, leading_quote);
    for block in &mut blocks {
        if let Block::Paragraph(inline) | Block::Heading(_, inline) = block {
            inline.source = start;
        }
    }
    (blocks, blanks, preedit.is_some())
}

impl Cx<'_, '_> {
    /// `block` 是否在光标所在的叶子块里（按块里第一处有原文对照的位置判断）。
    pub(super) fn raw_state(&mut self, block: &Block) -> RawState {
        let Some(raw) = &mut self.raw else {
            return RawState::Laid;
        };
        let start = match block {
            Block::Paragraph(inline) | Block::Heading(_, inline) => {
                (!inline.map.is_empty()).then_some(inline.source)
            }
            Block::Code { map, .. } => map.source(0),
            Block::Rule(at) => *at,
            Block::Table(_, rows, _) => rows
                .iter()
                .flatten()
                .find(|cell| !cell.map.is_empty())
                .map(|cell| cell.source),
            _ => None,
        };
        match start {
            Some(start) if raw.leaf.start <= start && start <= raw.parsed_end => {
                if std::mem::replace(&mut raw.done, true) {
                    // HTML 块生成的其余各块排在代码框下方，作为该块的渲染结果。
                    return match raw.kind {
                        RawKind::Html => RawState::Laid,
                        _ => RawState::Skip,
                    };
                }
                // 表格照常排，只有光标所在的那一格显示原文。
                if let Block::Table(_, rows, _) = block
                    && let Some(cell) = self.live_cell(rows)
                {
                    self.cell = Some(cell);
                    return RawState::Laid;
                }
                if matches!(block, Block::Paragraph(_) | Block::Heading(..))
                    && let Some(raw) = self.raw.as_ref().filter(|r| r.kind == RawKind::Prose)
                {
                    let (revealed, blanks, placed) = reveal(raw);
                    self.preedit |= placed;
                    return RawState::Reveal(revealed, blanks);
                }
                if matches!(block, Block::Rule(_)) {
                    return RawState::Rule;
                }
                RawState::Show
            }
            _ => RawState::Laid,
        }
    }

    /// 返回光标所在的表格（`rows` 是解析出的各行）里光标所在的单元格。
    ///
    /// 按现在的原文切分（见 `winc_preview::markdown::cells`）；光标在表头下面那一行上、原文的
    /// 行数或列数与解析结果对不上（加了一行、一列还没解析）时为 `None`，整张表显示原文。
    pub(crate) fn live_cell(&self, rows: &[Vec<Inline>]) -> Option<LiveCell> {
        use winc_preview::markdown::cells;
        let raw = self.raw.as_ref()?;
        let text = raw.block.get(raw.leaf.clone())?;
        let mut start = raw.leaf.start;
        let mut row = 0;
        let mut found = None;
        for line in text.split('\n') {
            let end = start + line.len();
            let here = (start..=end).contains(&raw.caret);
            if cells::is_delimiter(line) {
                if here {
                    return None;
                }
            } else if !line.trim().is_empty() {
                if here {
                    let cells = cells::cells(line);
                    let col = cells
                        .iter()
                        .position(|c| raw.caret - start <= c.segment.end)?;
                    let segment = &cells[col].segment;
                    found = Some((row, col, start + segment.start..start + segment.end));
                }
                row += 1;
            }
            start = end + 1;
        }
        let (r, c, range) = found?;
        (row == rows.len() && c < rows[r].len()).then_some((r, c, range))
    }

    /// 排版光标所在叶子块的原文。
    ///
    /// 公式、mermaid 图和 HTML 的下方另外绘制渲染结果，编辑时可以直接查看效果。
    /// 渲染结果来自最近一次的解析结果，可能比原文滞后一个解析周期。
    pub fn raw_with_preview(
        &mut self,
        block: &Block,
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Flow {
        let can_patch = env.depth == 0
            && x == 0.0
            && y == 0.0
            && out.items.is_empty()
            && self
                .raw
                .as_ref()
                .is_some_and(|r| matches!(&r.kind, RawKind::Fenced(lang) if !matches!(&**lang, "mermaid" | "latex")));
        let flow = if can_patch {
            if let Some(RawKind::Fenced(lang)) = self.raw.as_ref().map(|r| r.kind.clone()) {
                self.raw_box_into(&lang, x, y, width, true, out)
            } else {
                self.raw(x, y, width, &mut out.items)
            }
        } else {
            self.raw(x, y, width, &mut out.items)
        };
        let boxed = self
            .raw
            .as_ref()
            .is_some_and(|r| matches!(r.kind, RawKind::Fenced(_) | RawKind::Html));
        let gap = (self.base * 0.75).round();
        // 代码框自带底色，与下方的渲染结果之间不再绘制分隔线。
        let below = if boxed {
            y + flow.height + gap
        } else {
            y + flow.height + gap * 2.0 + 1.0
        };
        let html = self.raw.as_ref().is_some_and(|r| r.kind == RawKind::Html);
        let first = out.items.len() as u32;
        let preview = match block {
            Block::Math(..) => self.preview(block, x, below, width, env, out),
            Block::Code { lang, text, .. } if lang == "mermaid" => {
                self.diagram(text, x, below, width, env, &mut out.items)
            }
            _ if html => self.preview(block, x, below, width, env, out),
            _ => None,
        };
        let Some(preview) = preview.filter(|p| p.height > 0.0) else {
            return flow;
        };
        out.preview = Some((first, below));
        if !boxed {
            // 原文与渲染结果之间绘制一条细分隔线。
            out.items.push(Item::Fill {
                rect: Rect::new(x, y + flow.height + gap, width, 1.0),
                color: self.colors.border,
                radius: 0.0,
            });
        }
        Flow {
            height: below + preview.height - y,
            first_baseline: flow.first_baseline,
        }
    }

    /// 在 `(x, y)` 处按排版显示 `block`（不显示原文），作为原文下方的渲染结果。
    pub(super) fn preview(
        &mut self,
        block: &Block,
        x: f32,
        y: f32,
        width: f32,
        env: Env,
        out: &mut Out,
    ) -> Option<Flow> {
        let raw = self.raw.take();
        let flow = self.block(block, x, y, width, env, out);
        self.raw = raw;
        Some(flow)
    }

    /// 排版光标所在叶子块的原文（见 [`Raw`]）。
    ///
    /// 代码、公式、HTML 放入代码框（见 `Cx::raw_box`）；其余块使用等宽字体并按 Markdown 着色，
    /// 保留原有换行，过长时在字符之间折行。每段文字的原文位置即其在原文中的位置。
    pub fn raw(&mut self, x: f32, y: f32, width: f32, out: &mut Vec<Item>) -> Flow {
        let Some(raw) = &self.raw else {
            return Flow {
                height: 0.0,
                first_baseline: None,
            };
        };
        match &raw.kind {
            RawKind::Fenced(lang) => {
                let lang = lang.clone();
                return self.raw_box(&lang, x, y, width, out);
            }
            RawKind::Html => return self.raw_box("html", x, y, width, out),
            RawKind::Prose | RawKind::Plain => {}
        }
        let (block, leaf) = (Arc::clone(&raw.block), raw.leaf.clone());
        let text = block.get(leaf.clone()).unwrap_or_default();
        let size = self.base * 0.85;
        let line_h = (size * 1.45).round();
        let m = self.frame.metrics(FontKind::Mono, size);
        let row = Row {
            room: width.max(size),
            size,
            line_h,
            ascent: m.ascent,
            descent: m.descent,
        };
        let language = winc_syntax::find("markdown");
        let mut state = winc_syntax::LineState::default();
        let (mut top, mut first_baseline) = (y, None);
        let mut start = leaf.start;
        for line in text.split('\n') {
            let source = u32::try_from(start).unwrap_or(0);
            start += line.len() + 1;
            let piece = self.code_piece(line, source, "markdown", language, &mut state, row);
            first_baseline.get_or_insert(top + piece.first_baseline.unwrap_or(0.0));
            self.paste(&piece, x, top, source, out);
            top += piece.height;
        }
        Flow {
            height: top - y,
            first_baseline,
        }
    }

    /// 排版光标所在的分隔线：原文（`---`）以次要色写在左侧，分隔线从原文之后画到右边缘。
    ///
    /// 高度与排版后的分隔线相同，原文的一行在分隔线上下的外边距中垂直居中：光标移入、移出
    /// 分隔线时其下方的内容不移动，修改原文时仍可看出这是一条分隔线。
    pub(super) fn raw_rule(&mut self, x: f32, y: f32, width: f32, out: &mut Out) -> Flow {
        let h = rule_height(self.base);
        let flow = Flow {
            height: h,
            first_baseline: None,
        };
        let Some(raw) = &self.raw else {
            return flow;
        };
        let (block, leaf) = (Arc::clone(&raw.block), raw.leaf.clone());
        let text = block.get(leaf.clone()).unwrap_or_default();
        let line = text.split('\n').next().unwrap_or_default();
        let line = line.trim_end_matches('\r');
        let size = self.base * 0.85;
        let m = self.frame.metrics(FontKind::Mono, size);
        let row = Row {
            room: width.max(size),
            size,
            line_h: (size * 1.45).round(),
            ascent: m.ascent,
            descent: m.descent,
        };
        let language = winc_syntax::find("markdown");
        let mut state = winc_syntax::LineState::default();
        let source = u32::try_from(leaf.start).unwrap_or(0);
        let piece = self.code_piece(line, source, "markdown", language, &mut state, row);
        let first = out.items.len();
        self.paste(
            &piece,
            x,
            y + (h - piece.height) / 2.0,
            source,
            &mut out.items,
        );
        let mut right = x;
        for item in &mut out.items[first..] {
            right = right.max(item.span_x().1);
            if let Item::Text { color, .. } = item {
                *color = self.colors.muted;
            }
        }
        let gap = (self.base * 0.6).round();
        if right + gap < x + width {
            out.items.push(Item::Fill {
                rect: Rect::new(right + gap, y, x + width - right - gap, h),
                color: self.colors.border,
                radius: 0.0,
            });
        }
        flow
    }

    /// 将光标所在的代码、公式或 HTML 块的原文放入代码框，内容按 `lang` 着色。
    ///
    /// 代码框的底色、边距、字号与排版后的代码块相同；框后光标所在的空行排在框外。围栏行只在
    /// 光标位于其上时显示，并以较淡的颜色绘制。因此点击进入代码块时外观不变、文字不移动；
    /// 用 ←→ 移出内容首尾时显示围栏，可以修改语言标识。
    pub(super) fn raw_box(
        &mut self,
        lang: &str,
        x: f32,
        y: f32,
        width: f32,
        out: &mut Vec<Item>,
    ) -> Flow {
        let mut tmp = Out {
            items: std::mem::take(out),
            ..Out::default()
        };
        let flow = self.raw_box_into(lang, x, y, width, false, &mut tmp);
        *out = tmp.items;
        flow
    }

    pub(super) fn raw_box_into(
        &mut self,
        lang: &str,
        x: f32,
        y: f32,
        width: f32,
        record_patch: bool,
        out: &mut Out,
    ) -> Flow {
        let Some(raw) = &self.raw else {
            return Flow {
                height: 0.0,
                first_baseline: None,
            };
        };
        let (block, leaf) = (Arc::clone(&raw.block), raw.leaf.clone());
        let fenced = matches!(raw.kind, RawKind::Fenced(_));
        let blanks = blanks(raw);
        let text = block.get(leaf.clone()).unwrap_or_default();
        let text = &text[..text.trim_end().len()];
        let pad = self.base;
        let size = self.base * 0.85;
        let m = self.frame.metrics(FontKind::Mono, size);
        let row = Row {
            room: (width - pad * 2.0).max(size),
            size,
            line_h: (size * 1.45).round(),
            ascent: m.ascent,
            descent: m.descent,
        };
        let language = winc_syntax::find(lang);
        let markdown = winc_syntax::find("markdown");
        let mut state = winc_syntax::LineState::default();
        let bg_at = out.items.len();
        // 先占位底色矩形，避免排完数万行后 `Vec::insert(0, ...)` 整体搬移所有项。
        out.items.push(Item::Fill {
            rect: Rect::new(x, y, width, 0.0),
            color: self.colors.code_block,
            radius: 6.0,
        });
        let (mut top, mut first_baseline) = (y + pad, None);
        let mut start = leaf.start;
        let count = text.split('\n').count();
        let record = record_patch
            && fenced
            && !matches!(lang, "mermaid" | "latex")
            && count >= block::HUGE_CODE_LINES
            && blanks.is_empty();
        let mut line_spans = if record {
            Vec::with_capacity(count)
        } else {
            Vec::new()
        };
        let caret = raw.caret;
        for (i, line) in text.split('\n').enumerate() {
            let source = u32::try_from(start).unwrap_or(0);
            let source_end = u32::try_from(start + line.len()).unwrap_or(0);
            let here = (start..=start + line.len()).contains(&caret);
            start += line.len() + 1;
            // 第一行，以及（行数不少于两行且确为围栏时）最后一行是围栏，按 Markdown 着色。
            let fence = fenced && count > 1 && (i == 0 || (i + 1 == count && is_fence(line)));
            let line_top = top;
            let item_start = out.items.len() as u32;
            let tb_start = self.text.len();
            let ch_start = self.chars;
            if fence && !here {
                if record {
                    line_spans.push(block::CodeLineSpan {
                        items: item_start..item_start,
                        text_bytes: tb_start..tb_start,
                        chars: ch_start..ch_start,
                        source_start: source,
                        source_end,
                        top: line_top,
                        height: 0.0,
                        end_state: state,
                        fence,
                        shown: false,
                    });
                }
                continue;
            }
            let piece = if fence {
                let mut fence_state = winc_syntax::LineState::default();
                self.code_piece(line, source, "markdown", markdown, &mut fence_state, row)
            } else {
                self.code_piece(line, source, lang, language, &mut state, row)
            };
            first_baseline.get_or_insert(top + piece.first_baseline.unwrap_or(0.0));
            let at = out.items.len();
            self.paste(&piece, x + pad, top, source, &mut out.items);
            if fence {
                // 围栏只是标记，以较淡的颜色绘制。
                let muted = self.colors.muted;
                for item in &mut out.items[at..] {
                    if let Item::Text { color, .. } = item {
                        *color = muted;
                    }
                }
            }
            top += piece.height;
            if record {
                line_spans.push(block::CodeLineSpan {
                    items: item_start..out.items.len() as u32,
                    text_bytes: tb_start..self.text.len(),
                    chars: ch_start..self.chars,
                    source_start: source,
                    source_end,
                    top: line_top,
                    height: piece.height,
                    end_state: state,
                    fence,
                    shown: true,
                });
            }
        }
        let height = top + pad - y;
        let rect = Rect::new(x, y, width, height);
        out.items[bg_at] = Item::Fill {
            rect,
            color: self.colors.code_block,
            radius: 6.0,
        };
        // 代码块在编辑时同样显示复制按钮，复制围栏之间的代码（直接按字节切片，避免拷贝整块行表）。
        let mut code_item = None;
        if fenced && !matches!(lang, "mermaid" | "latex") {
            let body = block::fenced_body(text);
            code_item = Some(out.items.len() as u32);
            out.items.push(Item::Code {
                rect,
                text: body.into(),
                diagram: false,
            });
        }
        if record {
            out.patch = Some(Box::new(block::PatchIndex::CodeBox(block::CodeBoxPatch {
                lang: lang.into(),
                width,
                pad,
                row,
                bg_item: bg_at as u32,
                code_item,
                lines: line_spans,
                raw_len: text.len(),
                leaf_start: leaf.start,
            })));
        }
        let mut blank_out = Out::default();
        let extra = self.blank_lines(&blanks, x, y + height, width, &mut blank_out);
        out.items.append(&mut blank_out.items);
        Flow {
            height: height + extra,
            first_baseline,
        }
    }
}
