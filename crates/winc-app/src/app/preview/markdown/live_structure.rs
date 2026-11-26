//! 可编辑 Markdown 的原文结构适配器：将活动文档接入 `winc_richtext::SourceBridge`。

use ropey::Rope;
use winc_core::{Lines, Selection};
use winc_editor::Document;
use winc_richtext::{Fenced, LiveLeaf, SourceBridge};

use crate::app::App;

impl App {
    /// 构造当前活动文档的可编辑 Markdown 原文坐标桥。
    pub(super) fn live_source(&self) -> Option<SourceBridge<'_>> {
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        let parsed = self.preview.markdown.parsed.as_ref()?;
        let checked = self
            .preview
            .markdown
            .checked
            .filter(|_| self.preview.markdown.doc == Some(doc.id));
        Some(SourceBridge::new(rope, parsed, doc.revision, checked))
    }

    /// 可编辑的预览中文字光标的原文位置（字节偏移，按当前文字计算）。
    pub(crate) fn live_caret_source(&self) -> Option<usize> {
        let doc = self.editor.active()?;
        let rope = doc.text().rope()?;
        Some(rope.char_to_byte(doc.selection.head.min(rope.len_chars())))
    }

    /// 返回光标所在的围栏代码块或 `$$` 公式各行的位置；光标不在这类块中时为 `None`。
    pub(super) fn live_fenced(&self) -> Option<Fenced> {
        self.live_source()?.fenced_at(self.live_caret_source()?)
    }

    /// 返回可编辑的预览中光标所在的叶子块，见 [`LiveLeaf`]；尚未解析或没有叶子块时为 `None`。
    pub(crate) fn live_leaf(&self) -> Option<LiveLeaf> {
        self.live_leaf_at(self.live_caret_source()?)
    }

    pub(super) fn live_leaf_at(&self, caret: usize) -> Option<LiveLeaf> {
        self.live_source()?.leaf_at(caret)
    }

    /// 原文第 `at` 个字节所在的叶子块是否为正文（段落、标题）；不在任何叶子块中时为
    /// `Some(false)`，解析结果不是当前版本时为 `None`。
    pub(super) fn live_prose_at(&self, at: usize) -> Option<bool> {
        self.live_source()?.prose_at(at)
    }

    /// 解析落后时，只使用仍与已绘制内容及当前原文一致的段落范围。
    pub(super) fn paragraph_selection(
        &self,
        doc: &Document,
        rope: &Rope,
        source: usize,
    ) -> Option<Selection> {
        let preview = &self.preview.markdown;
        let checked = preview.checked.filter(|_| preview.doc == Some(doc.id))?;
        let parsed = preview.parsed.as_ref()?;
        // 命中位置来自上一帧，新解析可能还没画出来。
        if preview.layout.as_ref()?.source != preview.parse_serial {
            return None;
        }
        let bridge = SourceBridge::new(rope, parsed, doc.revision, Some(checked));
        bridge.paragraph_selection(
            source,
            |rev| doc.unchanged_lines_since(rev),
            |line| doc.text().line_span(line),
        )
    }
}
