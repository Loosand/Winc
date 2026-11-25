//! 预览：Markdown（后台解析、滚动、链接和图片）、图片和 SVG（后台解码、分栏预览）、表格（后台
//! 读取 xlsx、csv）、PDF（渲染线程逐页渲染）。
//!
//! 各种预览都在后台线程完成解码、解析和绘制，界面只收结果；排版与绘制在 `view` 模块中完成。

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use winc_editor::{PictureState, PreviewMode};
use winc_i18n::t;
use winc_preview::svg;

use crate::app::App;

pub(crate) mod images;
pub(crate) mod json;
pub(crate) mod markdown;
pub(crate) mod pdf;
pub(crate) mod tables;

/// 能在浏览器中打开的网页文件的扩展名（小写）。
const PAGE_EXTENSIONS: &[&str] = &["html", "htm", "xhtml"];

/// 使用系统字体绘制 SVG 中的文字。
///
/// 字体文件在第一次用到时才映射，之后常驻内存，所有 SVG 共用。
struct SystemGlyphs(winc_render::OutlineFonts);

impl svg::Glyphs for SystemGlyphs {
    fn glyph(&self, families: &[String], weight: u16, italic: bool, c: char) -> Option<svg::Glyph> {
        self.0
            .glyph(families, weight, italic, c)
            .map(|g| svg::Glyph {
                outline: g.outline,
                advance: g.advance,
            })
    }

    fn metrics(&self, families: &[String], weight: u16, italic: bool) -> svg::FontMetrics {
        let m = self.0.metrics(families, weight, italic);
        svg::FontMetrics {
            ascent: m.ascent,
            descent: m.descent,
            x_height: m.x_height,
        }
    }
}

/// 返回所有 SVG 共用的系统字体字形来源，首次调用时创建。
pub(crate) fn system_glyphs() -> Arc<dyn svg::Glyphs> {
    static GLYPHS: OnceLock<Arc<SystemGlyphs>> = OnceLock::new();
    GLYPHS
        .get_or_init(|| Arc::new(SystemGlyphs(winc_render::OutlineFonts::new())))
        .clone()
}

impl App {
    /// 当前标签是本机磁盘上的网页文件时返回它的路径（标签栏右端显示「在浏览器中打开」）。
    ///
    /// 远端主机上的文件没有本机浏览器可以打开的地址。
    pub(crate) fn browser_page(&self) -> Option<&Path> {
        let doc = self.editor.active()?;
        if doc.unsupported.is_some() || doc.title.is_some() {
            return None;
        }
        let path = doc
            .path
            .as_deref()
            .filter(|p| !winc_backend::is_remote(p))?;
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        PAGE_EXTENSIONS
            .contains(&extension.as_str())
            .then_some(path)
    }

    /// 用默认浏览器打开当前的网页文件。
    ///
    /// 浏览器读的是磁盘上的文件：有未保存的修改时在状态栏提示。
    pub(in crate::app) fn open_in_browser(&mut self) {
        let Some(path) = self.browser_page().map(Path::to_path_buf) else {
            return;
        };
        let dirty = self.editor.active().is_some_and(|d| d.is_dirty());
        match crate::system::open_in_browser(&path) {
            Ok(()) if dirty => self.flash(t!("preview-browser-unsaved")),
            Ok(()) => {}
            Err(e) => self.flash(t!("preview-browser-failed", error = e.to_string())),
        }
    }

    /// 当前标签换成 `mode` 显示（标签栏右端的三段切换）。
    pub(in crate::app) fn set_preview_mode(&mut self, mode: PreviewMode) {
        // 换了显示方式，键盘先回到文字（只看预览时本来就对着预览）。
        self.preview.markdown.focused = false;
        let top = self.leave_markdown_mode(mode);
        let Some(doc) = self.editor.active_mut() else {
            return;
        };
        // 分栏和预览解析的是同一段文字：版本没变时把已经解析好的图交过去，不再解析一遍，
        // 只按新的大小重画。
        let (id, revision) = (doc.id, doc.revision);
        let from_split = self.preview.split.ready_picture(id, revision);
        let from_preview = doc.picture().cloned();
        if !doc.set_preview_mode(mode) {
            self.flash(winc_i18n::t!("preview-mode-unsupported"));
            return;
        }
        match mode {
            PreviewMode::Preview => {
                if let Some(picture) = from_split {
                    doc.picture = Some(PictureState::Ready(picture));
                }
            }
            PreviewMode::Split => {
                if let Some(picture) = from_preview {
                    self.preview.split.seed_from_preview(id, revision, picture);
                }
            }
            PreviewMode::Source => {}
        }
        self.relayout();
        // 从只看预览换回来：文字滚到预览顶上的那一处，不回到换走之前的位置。
        if let Some(line) = top {
            self.scroll_text_to(line);
        }
        self.request_redraw();
    }
}

/// 各类预览的状态：图片、表格、分栏、Markdown 与 PDF。
pub(crate) struct PreviewState {
    /// 图片预览缩放后的图片缓存。
    pub(crate) picture_cache: Option<images::PictureCache>,
    /// 当前图片预览的缩放倍数与滚动位置。
    pub(crate) picture_view: images::PictureView,
    /// 后台的图片解码和 SVG 绘制任务。
    pub(in crate::app) images: images::ImageJobs,
    /// 后台的表格读取任务。
    pub(in crate::app) tables: tables::TableJobs,
    /// 表格预览上次绘制的几何：工作表标签、最大滚动距离和列宽缓存。
    pub(crate) table_layout: tables::TableLayout,
    /// 分栏预览右侧的图片。
    pub(crate) split: images::SplitPreview,
    /// JSON 结构图（分栏右侧或仅预览模式）。
    pub(crate) json: json::JsonView,
    /// Markdown 预览（分栏右侧或仅预览模式）。
    pub(crate) markdown: markdown::MarkdownPreview,
    /// 当前标签的 PDF 预览。
    pub(crate) pdf: pdf::PdfPreview,
    /// 分栏预览时源代码一侧占编辑区宽度的比例，取值 0 到 1；`None` 表示各占一半。
    ///
    /// 拖动中间的分隔线时改变，所有分栏预览的标签共用该值。
    pub(in crate::app) split_share: Option<f32>,
}

/// 随窗格走的预览状态：拆分后每个窗格各有一份，见 [`PreviewState::exchange`]。
#[derive(Default)]
pub(crate) struct PanePreview {
    picture_cache: Option<images::PictureCache>,
    picture_view: images::PictureView,
    table_layout: tables::TableLayout,
    split: images::SplitPreview,
    json: json::JsonView,
    markdown: markdown::MarkdownPreview,
    pdf: pdf::PdfPreview,
}

impl std::fmt::Debug for PanePreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PanePreview")
    }
}

impl PanePreview {
    /// 这个窗格正在预览的 PDF 文件和它此刻的阅读位置。
    pub(crate) fn pdf_place(&self) -> Option<(&PathBuf, &pdf::Place)> {
        self.pdf.current_place()
    }
}

impl PreviewState {
    /// 和 `other` 交换随窗格走的预览状态；PDF 和书的阅读位置不随窗格走。
    pub(crate) fn exchange(&mut self, other: &mut PanePreview) {
        std::mem::swap(&mut self.picture_cache, &mut other.picture_cache);
        std::mem::swap(&mut self.picture_view, &mut other.picture_view);
        std::mem::swap(&mut self.table_layout, &mut other.table_layout);
        std::mem::swap(&mut self.split, &mut other.split);
        std::mem::swap(&mut self.json, &mut other.json);
        std::mem::swap(&mut self.markdown, &mut other.markdown);
        std::mem::swap(&mut self.pdf, &mut other.pdf);
        self.markdown.trade_places(&mut other.markdown);
        self.pdf.trade_places(&mut other.pdf);
    }
}
