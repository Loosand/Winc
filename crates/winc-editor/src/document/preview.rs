//! 文档预览：图片、SVG、Markdown 与表格的呈现、编辑区与预览之间的切换，以及解码好的图片、
//! 读好的表格等派生状态。

use super::*;

/// 文档标签页在编辑区主区域呈现的视图形态（按唯一权威优先级判定）。
#[derive(Debug)]
pub enum TabSurface<'a> {
    /// 图像文件或切换到预览的 SVG。
    Picture(&'a PictureState),
    /// docx / EPUB 等转为 Markdown 排版显示的阅读文档。
    ConvertedMarkdown,
    /// 阅读标签页（笔记库主页或后台取回的 Markdown）。
    Reading(&'a Reading),
    /// PDF 逐页预览。
    Pdf,
    /// 只读表格视图（xlsx 工作簿，或仅预览的 csv / tsv）。
    Table {
        /// 表格视图状态。
        view: &'a TableView,
        /// 是否为无源码工作簿（xlsx）。
        workbook: bool,
    },
    /// 可直接编辑的 Markdown 排版预览。
    MarkdownPreview,
    /// JSON 结构图只读预览。
    JsonPreview,
    /// 并排或单栏差异视图。
    Diff,
    /// Git 日志视图。
    Log,
    /// 十六进制字节视图。
    Hex,
    /// 正在后台读取中的占位标签。
    Loading,
    /// 无法作为文本打开的文件说明页。
    Unsupported(&'a Unsupported),
    /// 普通源代码 / 文本编辑视图（含分栏预览的左侧编辑区）。
    Text,
}

impl Document {
    /// 按唯一权威优先级返回该文档在编辑区主区域应呈现的视图形态。
    pub fn tab_surface(&self) -> TabSurface<'_> {
        if let Some(picture) = self.picture.as_ref() {
            return TabSurface::Picture(picture);
        }
        if self
            .unsupported
            .as_ref()
            .is_some_and(Unsupported::reads_as_markdown)
        {
            return TabSurface::ConvertedMarkdown;
        }
        if let Some(reading) = self.reading.as_ref() {
            return TabSurface::Reading(reading);
        }
        if self.unsupported == Some(Unsupported::Pdf) {
            return TabSurface::Pdf;
        }
        if let Some(view) = self.table.as_deref().filter(|_| !self.shows_text()) {
            return TabSurface::Table {
                view,
                workbook: self.unsupported.is_some(),
            };
        }
        if self.preview_kind() == Some(PreviewKind::Markdown)
            && self.preview_mode() == PreviewMode::Preview
        {
            return TabSurface::MarkdownPreview;
        }
        if self.preview_kind() == Some(PreviewKind::Json)
            && self.preview_mode() == PreviewMode::Preview
            && !self.pending
        {
            return TabSurface::JsonPreview;
        }
        if self.diff.is_some() {
            return TabSurface::Diff;
        }
        if self.log.is_some() {
            return TabSurface::Log;
        }
        if self.hex.is_some() {
            return TabSurface::Hex;
        }
        if self.pending {
            return TabSurface::Loading;
        }
        if let Some(reason) = self.unsupported.as_ref() {
            return TabSurface::Unsupported(reason);
        }
        TabSurface::Text
    }

    /// 返回编辑区显示的图；尚未解码完成或没有图时返回 `None`。
    pub fn picture(&self) -> Option<&Arc<Picture>> {
        match &self.picture {
            Some(PictureState::Ready(picture)) => Some(picture),
            _ => None,
        }
    }

    /// 是否在编辑区显示文本：不是差异、日志、阅读标签或无法打开的文件，且没有只显示预览。
    ///
    /// 光标、查找、折叠等只对文本有意义。
    pub fn shows_text(&self) -> bool {
        self.unsupported.is_none()
            && self.picture.is_none()
            && (self.table.is_none() || self.split)
            && !self.rendered
            && !self.json_preview
            && self.diff.is_none()
            && self.log.is_none()
            && self.hex.is_none()
            && self.reading.is_none()
    }

    /// 是否能编辑文本：显示文本（见 [`Document::shows_text`]），或者是 Markdown 的预览（预览
    /// 上直接编辑，光标所在的块显示原文）。
    ///
    /// 编辑命令、粘贴、查找按它判断；只与编辑区的文本区有关的（折叠、粘滞滚动、逐行追溯）
    /// 仍按 [`Document::shows_text`]。
    pub fn edits_text(&self) -> bool {
        self.shows_text() || self.rendered
    }

    /// 是否可以切换到预览，见 [`Document::preview_kind`]。
    pub fn can_preview(&self) -> bool {
        self.preview_kind().is_some()
    }

    /// 能预览时是哪一种；不能预览时返回 `None`（见 [`Document::can_preview`]）。
    ///
    /// 按文档当前的语言判断，与文件名无关：未保存的新文件和扩展名不符的文件选择语言后同样可以预览。
    pub fn preview_kind(&self) -> Option<PreviewKind> {
        if self.unsupported.is_some() || self.title.is_some() {
            return None;
        }
        let kind = PreviewKind::of(self.highlighter.language())?;
        // 大文件的原文不在内存中：只有表格可以预览，内容取自磁盘上的文件。
        (!self.is_large() || kind == PreviewKind::Table).then_some(kind)
    }

    /// 是否正在只看预览。
    pub(super) fn previewing(&self) -> bool {
        self.picture.is_some()
            || self.rendered
            || self.json_preview
            || (self.table.is_some() && !self.split)
    }

    /// 文本是否可以编辑。
    ///
    /// 与 `readonly` 不同：正在预览的 SVG 在预览期间暂时只读，文本仍可编辑，切回源代码即可
    /// 修改；未保存的修改仍需备份，也可以换成磁盘版本。
    pub fn text_editable(&self) -> bool {
        if self.previewing() && self.can_preview() {
            !self.readonly_before_preview
        } else {
            !self.readonly
        }
    }

    /// 返回文档的显示方式：不能预览的文件总是 [`PreviewMode::Source`]。
    pub fn preview_mode(&self) -> PreviewMode {
        if !self.can_preview() {
            PreviewMode::Source
        } else if self.previewing() {
            PreviewMode::Preview
        } else if self.split {
            PreviewMode::Split
        } else {
            PreviewMode::Source
        }
    }

    /// 换成 `mode` 显示。
    ///
    /// 只看 SVG、表格或 JSON 的预览时文档只读，离开时恢复原来的只读状态；Markdown 的
    /// 预览可以直接编辑，不改只读状态。不满足 [`Document::can_preview`] 时不切换，返回 `false`。
    pub fn set_preview_mode(&mut self, mode: PreviewMode) -> bool {
        let Some(kind) = self.preview_kind() else {
            return false;
        };
        if self.previewing() {
            self.picture = None;
            self.rendered = false;
            self.json_preview = false;
            self.readonly = self.readonly_before_preview;
        }
        // 表格在只看预览和分栏时都有，换显示方式时一律重新读。
        self.table = None;
        self.split = mode == PreviewMode::Split;
        if self.split && kind == PreviewKind::Table {
            self.table = Some(TableView::loading(self.revision));
        }
        if mode == PreviewMode::Preview {
            self.readonly_before_preview = self.readonly;
            if kind != PreviewKind::Markdown {
                self.readonly = true;
            }
            match kind {
                PreviewKind::Svg => self.picture = Some(PictureState::Loading),
                PreviewKind::Markdown => self.rendered = true,
                PreviewKind::Table => self.table = Some(TableView::loading(self.revision)),
                PreviewKind::Json => self.json_preview = true,
            }
        }
        true
    }

    /// 在 SVG 的源代码（或分栏）和只看预览之间切换，返回切换后是否处于预览。
    ///
    /// 不满足 [`Document::can_preview`] 时不切换，返回 `false`。
    pub fn toggle_preview(&mut self) -> bool {
        let mode = match self.preview_mode() {
            PreviewMode::Preview => PreviewMode::Source,
            PreviewMode::Source | PreviewMode::Split => PreviewMode::Preview,
        };
        self.set_preview_mode(mode) && mode == PreviewMode::Preview
    }

    /// 释放已经解码好的图，返回是否释放了；再次显示时重新解码（[`PictureState::Loading`]）。
    ///
    /// 用于不在屏幕上的标签：解码后的图片最多约 26 MB（2560 × 2560 RGBA）。
    pub fn release_picture(&mut self) -> bool {
        if !matches!(self.picture, Some(PictureState::Ready(_))) {
            return false;
        }
        self.picture = Some(PictureState::Loading);
        true
    }

    /// 接收后台解码或绘制完成的结果，返回是否采用。
    ///
    /// `revision` 是生成该结果所依据的文档版本。版本已变化（例如文件被重新读取），或当前不在
    /// [`PictureState::Loading`] 状态时丢弃结果并返回 `false`。图片文件解码失败时，文档转为
    /// 无法读取的文件（`unsupported`），用户可以「重试」。
    pub fn set_picture(&mut self, revision: u64, result: Result<Picture, String>) -> bool {
        if revision != self.revision || !matches!(self.picture, Some(PictureState::Loading)) {
            return false;
        }
        match result {
            Ok(picture) => self.picture = Some(PictureState::Ready(Arc::new(picture))),
            Err(reason) if self.unsupported.is_some() => {
                self.picture = None;
                self.unsupported = Some(Unsupported::Unreadable(reason));
            }
            Err(reason) => self.picture = Some(PictureState::Failed(reason)),
        }
        true
    }

    /// 释放读好的表格，返回是否释放了；再次显示时重新读取（[`TableState::Loading`]），工作表
    /// 和滚动位置保留。
    ///
    /// 用于不在屏幕上的标签：读好的表格最多有上百万个单元格。
    pub fn release_table(&mut self) -> bool {
        match &mut self.table {
            Some(view) if matches!(view.state, TableState::Ready(_)) => {
                view.state = TableState::Loading;
                true
            }
            _ => false,
        }
    }

    /// 需要（重新）读取表格时返回依据的文档版本；表格已经是最新的时返回 `None`。
    ///
    /// 尚未读取过，或者读好的表格落后于文档时返回当前版本，例如文本被编辑过、表格文件在磁盘上
    /// 已改变。读好的表格在按新版本读好之前继续显示。
    pub fn table_to_load(&self) -> Option<u64> {
        let view = self.table.as_ref()?;
        let stale = matches!(view.state, TableState::Loading) || view.revision != self.revision;
        stale.then_some(self.revision)
    }

    /// 接收后台读好的表格，返回是否采用。
    ///
    /// `revision` 是读取所依据的文档版本；文档已经又变了时丢弃结果并返回 `false`，随后按最新的
    /// 版本再读（见 [`Document::table_to_load`]）。
    pub fn set_table(&mut self, revision: u64, result: Result<Table, String>) -> bool {
        let Some(view) = &mut self.table else {
            return false;
        };
        if revision != self.revision {
            return false;
        }
        view.revision = revision;
        view.state = match result {
            Ok(table) => {
                view.sheet = view.sheet.min(table.sheets.len().saturating_sub(1));
                TableState::Ready(Arc::new(table))
            }
            Err(reason) => TableState::Failed(reason),
        };
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_surface_dispatches_preview_and_special_tab_precedence() {
        let mut doc = Document::from_text(DocumentId(1), "# 标题\n");
        assert!(matches!(doc.tab_surface(), TabSurface::Text));

        doc.set_language("a.md");
        assert!(doc.set_preview_mode(PreviewMode::Preview));
        assert!(matches!(doc.tab_surface(), TabSurface::MarkdownPreview));
        assert!(doc.set_preview_mode(PreviewMode::Split));
        assert!(matches!(doc.tab_surface(), TabSurface::Text));

        let mut csv = Document::from_text(DocumentId(2), "a,b\n1,2\n");
        csv.set_language("a.csv");
        assert!(csv.set_preview_mode(PreviewMode::Split));
        assert!(matches!(csv.tab_surface(), TabSurface::Text));
        assert!(csv.set_preview_mode(PreviewMode::Preview));
        assert!(matches!(
            csv.tab_surface(),
            TabSurface::Table {
                workbook: false,
                ..
            }
        ));

        let mut json = Document::from_text(DocumentId(3), "{\"a\":1}");
        json.set_language("a.json");
        assert!(json.set_preview_mode(PreviewMode::Preview));
        assert!(matches!(json.tab_surface(), TabSurface::JsonPreview));

        let pending = Document::pending(DocumentId(4), PathBuf::from("a.rs"));
        assert!(matches!(pending.tab_surface(), TabSurface::Loading));

        let reading = Document::reading(DocumentId(5), "页", Reading::Loading);
        assert!(matches!(
            reading.tab_surface(),
            TabSurface::Reading(Reading::Loading)
        ));

        let mut docx = Document::from_text(DocumentId(6), "");
        docx.unsupported = Some(Unsupported::Docx);
        assert!(matches!(docx.tab_surface(), TabSurface::ConvertedMarkdown));

        let mut pdf = Document::from_text(DocumentId(7), "");
        pdf.unsupported = Some(Unsupported::Pdf);
        assert!(matches!(pdf.tab_surface(), TabSurface::Pdf));

        let mut binary = Document::from_text(DocumentId(8), "");
        binary.unsupported = Some(Unsupported::Binary(128));
        assert!(matches!(
            binary.tab_surface(),
            TabSurface::Unsupported(Unsupported::Binary(128))
        ));
    }
}
