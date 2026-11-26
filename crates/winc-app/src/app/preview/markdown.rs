//! Markdown 预览（分栏的右半边，或只看预览时的整个编辑区）：后台解析、滚动、链接和图片。
//!
//! 同一时刻只在后台解析一个版本，输入快于解析时跳过中间的版本；可编辑的预览在解析足够快时
//! 改为绘制之前直接解析（见 `INLINE_PARSE`）。排版在绘制时进行（需要测量字宽），
//! 按块缓存，见 `view::preview::markdown`。图片读取本地文件（相对文档所在目录）；网址的图片
//! （README 中的徽章、截图）在后台用系统的 `curl` 下载到临时目录缓存，关闭联网后显示替代文字。

use crate::app::ScrollTarget;
use crate::app::scroll::set_scroll_line;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use winc_core::Lines;
use winc_editor::{DocumentId, PreviewKind, PreviewMode};
use winc_i18n::t;
use winc_preview::epub::TocEntry;
use winc_preview::image::{self, Picture, Pixels};
use winc_preview::markdown::{self, Markdown};

use super::system_glyphs;
use winit::keyboard::{Key, NamedKey};

use crate::app::{App, Focus, UserEvent};
use crate::system;

mod book;
mod images;
pub(crate) mod laid;
pub(crate) mod links;
mod live;
mod live_caret;
mod live_selection;
mod live_structure;
mod paste;
pub(crate) mod properties;
pub(crate) mod resize;
mod scroll;
pub(crate) mod slash;
pub(crate) mod suggest;
pub(crate) mod toolbar;
mod viewer;
mod writing;
pub(crate) use laid::{Fading, LiveLeaf, Moves};
pub(crate) use scroll::TocListLayout;
pub(crate) use viewer::{DiagramLaid, DiagramTool, DiagramView, MAX_ZOOM, MIN_ZOOM};
use winc_richtext::LiveState;
pub(crate) use winc_richtext::paragraph_lines;

/// docx、EPUB 包中的图片：Markdown 中引用的地址和图片文件的内容。
type Media = Vec<(String, Arc<[u8]>)>;

/// 转换 docx、EPUB 的结果：解析好的 Markdown、包中的图片和书的目录（docx 没有目录）。
type Converted = (Markdown, Media, Option<Arc<[TocEntry]>>);

/// 后台任务的结果。
enum Done {
    /// 文档、所依据的文档版本、解析结果、解析所用的时间。
    Parsed(DocumentId, u64, Markdown, std::time::Duration),
    /// docx、EPUB 文档、所依据的文档版本、转换并解析的结果或失败的原因。
    Converted(DocumentId, u64, Result<Converted, String>),
    /// 图片（解析后的本地路径）、读取结果。
    Loaded(PathBuf, Result<Arc<Picture>, String>),
    /// 图片、按显示大小（物理像素）绘制的像素。
    Drawn(PathBuf, Arc<Pixels>),
    /// 图片、未能绘制的大小（物理像素）：重新读取图片失败。
    Undrawn(PathBuf, (u32, u32)),
}

pub(crate) use winc_typeset::ImageSlot;

/// 可编辑的预览在界面线程上解析的时间预算：60 Hz 下一帧（约 16.7 ms）的四分之一。
///
/// 上一次解析不超过该预算时，输入后在绘制之前直接解析，同一帧即按最新的结构排版；超过时
/// 在后台解析，解析完成之前按上一版的结构显示。
const INLINE_PARSE: std::time::Duration = std::time::Duration::from_millis(4);

/// 首次切换到 Markdown 文档时，在界面线程上直接同步解析的体积上限（字节）。
///
/// 64 KB 以内的常见笔记通常在 1 ms 内解析完成（远低于 [`INLINE_PARSE`] 预算），
/// 直接同步解析即可在当帧一次性绘制新文档，避免切页时先闪出一帧空白；更大的文档仍在后台解析，
/// 并由 [`App::holding_frame`] 在首次解析完成前保留上一帧。
pub(crate) const INLINE_FIRST_PARSE_BYTES: usize = 64 * 1024;

/// 一张图片最多保留的已绘制尺寸数（种）；超出时移除最早绘制的尺寸。
const SIZES: usize = 4;
/// 按路径保留的滚动位置记录上限（写作模式关闭旧文档后再次打开同一篇笔记时恢复）。
const RECENT_DOC_STATES: usize = 64;
/// 切换文档后保留最近几篇笔记的排版缓存（Mermaid、公式、代码块等），切回时首帧复用。
const RECENT_MEMOS: usize = 3;

/// 预览左侧的标题指示条上可以指向的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TocTarget {
    /// 指示条本身：鼠标停在上面时展开标题列表。
    Bars,
    /// 标题列表里的一项，值为 [`MarkdownPreview::toc_tops`] 的序号：点击后滚动到该标题。
    Heading(usize),
    /// 标题列表右上角的图钉：钉住后列表一直展开，再点一次取消。
    Pin,
}

/// 当前标签的 Markdown 预览。
/// 可横向滚动的表格的滚动条，见 [`MarkdownPreview::table_bar`]。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TableBar {
    /// 表格：（顶层块序号，块中表格的序号）。
    pub key: (usize, usize),
    /// 滚动条的轨道（窗口坐标）：表格的显示区域，滑块位于其底部。
    pub track: winc_render::Rect,
    /// 表格的总宽度（逻辑像素）。
    pub content: f32,
}

pub(crate) struct MarkdownPreview {
    /// 上一帧的绘制区域（分栏的右半边或整个编辑区）；不显示时为空矩形。
    pub area: winc_render::Rect,
    /// 预览的文档；当前标签改变时重新开始。
    pub doc: Option<DocumentId>,
    /// 当前预览文档对应的路径（用于写作模式切回同一笔记时恢复滚动、属性折叠与排版缓存）。
    doc_path: Option<PathBuf>,
    /// 最近一次解析的结果。
    pub parsed: Option<Arc<Markdown>>,
    /// 解析结果的单调递增版本号（支持就地增量更新后仍唯一标识不同解析版本）。
    pub(crate) parse_serial: usize,
    /// 最新解析过的文档版本号（`revision`）。
    checked: Option<u64>,
    /// 正在后台解析的版本号（`revision`）。
    pending: Option<u64>,
    /// 首次为当前文档发起后台解析的时刻：在该文档拿到第一份解析结果前，用于配合
    /// [`App::holding_frame`] 保留上一帧，避免大文档切换时闪出一帧空白。
    pub(crate) first_parse_since: Option<(DocumentId, std::time::Instant)>,
    /// 最近一次解析当前文档所用的时间（含拼接整段文字）；尚未解析过时为 `None`。
    ///
    /// 决定可编辑的预览是否在界面线程上解析，见 [`INLINE_PARSE`]。
    pub(crate) cost: Option<std::time::Duration>,
    /// 各文档预览的滚动位置（逻辑像素），切换到其他标签再切回时保持不变。
    scrolls: HashMap<DocumentId, f32>,
    /// 按路径保留的滚动位置，供写作模式关闭并重新打开同一笔记时恢复。
    path_scrolls: HashMap<PathBuf, f32>,
    /// 排版结果，见 [`laid::Laid`]。
    pub layout: Option<crate::app::preview::markdown::laid::Laid>,
    pub(crate) property_layout_dirty: bool,
    pub(crate) property_collapsed: HashSet<DocumentId>,
    pub(crate) path_property_collapsed: HashSet<PathBuf>,
    pub(crate) property_source: HashSet<DocumentId>,
    pub(crate) imported_property_types: properties::ImportedTypes,
    /// 排版缓存，见 `winc_typeset::Memo`。
    pub memo: crate::app::preview::markdown::laid::Memo,
    /// 最近离开的文档的排版缓存，来回切换带图表/公式的笔记时直接复用。
    recent_memos: VecDeque<(PathBuf, crate::app::preview::markdown::laid::Memo)>,
    /// 图片缓存池（槽位、原始地址、内嵌媒体、待刷新队列和版本号）。
    pub(crate) images: images::ImagePool,
    /// docx、EPUB 读取或转换失败的原因；没有失败时为 `None`。
    pub failed: Option<String>,
    /// EPUB 的目录，在侧栏的大纲中列出；不是 EPUB 或尚未转换完时为 `None`。
    pub contents: Option<Arc<[TocEntry]>>,
    /// 各本书的阅读位置。
    book: book::BookPlaces,
    /// 被点击后开合状态与原文相反的折叠块（`<details>`），见 `view::markdown` 的 `DETAILS`。
    pub toggled: HashSet<u64>,
    /// 刚点击的折叠块标题的地址：只重排其所在的块。
    pub toggled_now: Option<String>,
    /// 点击 `其他.md#标题` 打开的文档与目标标题：该文档排版完成后滚动到该标题。
    pub pending_anchor: Option<(DocumentId, String)>,
    /// 最近一次横向滚动的表格：（顶层块序号，块中表格的序号），见 `view::markdown` 的
    /// `Laid::hscroll`。
    pub last_table: Option<(usize, usize)>,
    /// 左侧标题指示条与展开目录的状态机。
    pub(crate) toc: scroll::TocPanel,
    /// 分帧排版的状态。
    catch_up: scroll::CatchUp,
    /// 跳转到的脚注或脚注引用在正文中的底色：范围（上边缘和高度，相对内容顶部，逻辑像素）与
    /// 淡出的时钟。
    pub anchor_flash: Option<((f32, f32), crate::app::animation::Clock)>,
    /// 全屏查看中的图；未打开全屏查看时为 `None`。
    pub diagram: Option<DiagramView>,
    /// 鼠标所在代码块在 `codes` 中的序号。
    pub code_hover: Option<usize>,
    /// 鼠标所在图片的范围（窗口坐标）。
    pub(crate) image_hover: Option<winc_render::Rect>,
    /// 上一帧排版的总高度（逻辑像素），滚动时据此限制范围。
    pub height: f32,
    /// 选中的文字：起点和终点（整份文字里的字符偏移，见 `laid::Laid::text`）。
    pub selection: Option<(usize, usize)>,
    /// 最近一次点击是否在预览上：为真时分栏中的 ⌘C、⌘A、⌘F 作用于预览；点击左侧文字后
    /// 恢复为作用于文字。
    pub focused: bool,

    /// 查找用的文字：预览的整段文字装入一个不显示的文档，查找条按普通文档的方式查找。
    ///
    /// 重新排版时作废，需要查找时重新装入。
    pub find_doc: Option<winc_editor::Document>,
    /// `find_doc` 的装入次数：每次装入使用新的文档标识，原有的查找结果随之作废。
    find_serial: u64,
    /// 分栏时上一次两侧对齐的状态：文档、左侧文字的滚动行、预览的总高度。
    ///
    /// 左侧滚动（滚轮、光标移出屏幕、输入）或预览重排后，下一帧按左侧重新对齐右侧；
    /// 在右侧滚动时直接对齐左侧，并记录该状态。
    pub synced: Option<(DocumentId, f64, f32)>,
    /// 可编辑预览中拖动选择文字的状态。
    live_drag: Option<live_selection::LiveDrag>,
    /// 可编辑预览的交互与导航状态（空段收回、目标列、折行归属、编辑态和光标跟随）。
    pub(crate) live: LiveState<DocumentId>,
    /// 下一次重排时各块滑动到新位置（见 [`BlockMotion`]）：表格工具增删行列后置位。
    ///
    /// 输入文字引起的重排不滑动，内容直接到位。
    pub slide: bool,
    /// 刚切换为只看预览的文档：排版完成后预览滚动到文字原来的滚动位置，见
    /// [`App::align_markdown_once`]。
    align: Option<DocumentId>,
    /// 重排后的过渡动画：各块从原位置滑动到新位置，新显示的标记淡入，见 [`BlockMotion`]。
    pub motion: Option<BlockMotion>,
    /// 测试中固定的 [`BlockMotion`] 已用时间，用于截取动画中间帧。
    #[cfg(test)]
    pub frozen: Option<std::time::Duration>,
    sender: mpsc::Sender<Done>,
    receiver: mpsc::Receiver<Done>,
    /// 补全链接地址的列表。
    pub(crate) suggest: suggest::Suggest,
    /// 鼠标选中文字后的选区，选区不变时显示格式工具栏。
    pub(crate) toolbar: Option<(DocumentId, winc_core::Selection)>,
    /// 正文链接的检查结果。
    pub(crate) links: links::LinkChecks,
    /// 右键点击的本地图片文件，供菜单「在新标签中打开图片」使用。
    pub(crate) menu_picture: Option<PathBuf>,
    /// 右键点击的图片，供菜单「图片大小」使用。
    pub(crate) menu_image: Option<resize::ImageAt>,
    /// 正在拖动把手的图片。
    pub(crate) resizing: Option<resize::Resizing>,
    /// 非侵入式视图装饰层（搜索高亮、断链诊断、批注、AI 原位预览等），随编辑自动同步锚点。
    pub(crate) decorations: winc_richtext::DecorationSet,
    /// `decorations` 最近一次同步对应的文档版本号。
    decorations_revision: Option<u64>,
    /// `DecorationLayer::Search` 最近一次同步时的缓存键（文档、版本、匹配摘要、主题）。
    search_deco_key: Option<(DocumentId, u64, u64)>,
    /// `DecorationLayer::Diagnostic` 最近一次同步时的缓存键（文档、解析指针、校验代数、主题）。
    diag_deco_key: Option<(DocumentId, usize, u64, u32)>,
}

/// 可编辑的预览重排后的过渡动画。
///
/// 显示原文的块改变外观或表格删除一行后，各顶层块从原绘制位置滑动到新位置，高度变化的块按
/// 过渡中的高度裁剪；仅移动光标（未修改文字）时，新显示的标记和围栏淡入。输入文字时不淡入，
/// 否则每输入一个字符，标记都会闪烁一次。
pub(crate) struct BlockMotion {
    /// 过渡动画的时钟。
    pub clock: crate::app::Clock,
    /// 各顶层块（按新排版的序号）起始时相对新位置向下的偏移（逻辑像素，向上时为负）。
    pub from: Vec<f32>,
    /// 淡入的项。
    pub fading: Fading,
    /// 刚显示原文的块中渲染结果的偏移，见 [`Moves::inner`]。
    pub inner: Vec<(usize, u32, f32)>,
    /// 刚恢复排版的块原来的原文，在原处淡出。
    pub leaving: Vec<laid::Leaving>,
}

impl BlockMotion {
    /// 返回第 `block` 块恢复排版前的原文。
    pub fn leaving(&self, block: usize) -> Option<&laid::Leaving> {
        self.leaving.iter().find(|l| l.block == block)
    }

    /// 返回第 `block` 块中渲染结果的第一项和当前偏移（逻辑像素）。
    pub fn inner(&self, block: usize) -> Option<(usize, f32)> {
        let i = self.inner.binary_search_by_key(&block, |(b, ..)| *b).ok()?;
        let (_, first, d) = self.inner[i];
        Some((first as usize, d * (1.0 - self.clock.progress())))
    }

    /// 返回第 `block` 块当前相对新位置向下的偏移（逻辑像素）。
    pub fn offset(&self, block: usize) -> f32 {
        self.from
            .get(block)
            .map_or(0.0, |d| d * (1.0 - self.clock.progress()))
    }

    /// 是否仍有块未到达新位置。
    pub fn moving(&self) -> bool {
        self.from.iter().any(|d| d.abs() >= 0.5) || !self.inner.is_empty()
    }

    /// 返回第 `block` 块中淡入的项。
    pub fn fading(&self, block: usize) -> &[usize] {
        self.fading
            .binary_search_by_key(&block, |(b, _)| *b)
            .map_or(&[], |i| &self.fading[i].1)
    }
}

impl Default for MarkdownPreview {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            doc: None,
            doc_path: None,
            parsed: None,
            parse_serial: 0,
            checked: None,
            pending: None,
            first_parse_since: None,
            cost: None,
            scrolls: HashMap::new(),
            path_scrolls: HashMap::new(),
            layout: None,
            property_layout_dirty: false,
            property_collapsed: HashSet::new(),
            path_property_collapsed: HashSet::new(),
            property_source: HashSet::new(),
            imported_property_types: properties::ImportedTypes::default(),
            last_table: None,
            memo: Default::default(),
            recent_memos: VecDeque::new(),
            images: images::ImagePool::default(),
            failed: None,
            contents: None,
            book: book::BookPlaces::default(),
            toggled: HashSet::new(),
            toggled_now: None,
            pending_anchor: None,
            toc: scroll::TocPanel::default(),
            catch_up: scroll::CatchUp::default(),
            anchor_flash: None,
            diagram: None,
            code_hover: None,
            image_hover: None,
            area: winc_render::Rect::default(),
            height: 0.0,
            selection: None,
            focused: false,
            find_doc: None,
            find_serial: 0,
            synced: None,
            live_drag: None,
            live: LiveState::default(),
            slide: false,
            align: None,
            motion: None,
            #[cfg(test)]
            frozen: None,
            sender,
            receiver,
            suggest: Default::default(),
            links: Default::default(),
            toolbar: None,
            menu_picture: None,
            menu_image: None,
            resizing: None,
            decorations: winc_richtext::DecorationSet::new(),
            decorations_revision: None,
            search_deco_key: None,
            diag_deco_key: None,
        }
    }
}

impl MarkdownPreview {
    /// 标签 `id` 关闭后释放其按 ID 索引的滚动与属性面板状态（按路径记忆的仍保留）。
    pub(crate) fn forget(&mut self, id: DocumentId) {
        self.scrolls.remove(&id);
        self.property_collapsed.remove(&id);
        self.property_source.remove(&id);
    }

    /// 和 `other` 交换各本书的阅读位置。
    pub(super) fn trade_places(&mut self, other: &mut MarkdownPreview) {
        std::mem::swap(&mut self.book, &mut other.book);
    }

    /// 返回当前文档预览的滚动位置（逻辑像素）。
    pub fn scroll(&self) -> f32 {
        self.doc
            .and_then(|id| self.scrolls.get(&id))
            .copied()
            .unwrap_or(0.0)
    }

    /// 返回最大滚动位置（逻辑像素）：内容高于预览区域的部分，不小于 0。
    pub fn max_scroll(&self) -> f32 {
        (self.height - self.area.h).max(0.0)
    }

    /// 滚动到 `y`（逻辑像素），限制在内容范围内。
    pub fn scroll_to(&mut self, y: f32) {
        self.set_scroll(y.clamp(0.0, self.max_scroll()));
    }

    /// 把当前文档预览的滚动位置设为 `y`（逻辑像素），不限制范围。
    ///
    /// 供绘制前后换入、恢复过渡中的位置使用；其他情况用 [`Self::scroll_to`]。
    pub(crate) fn set_scroll(&mut self, y: f32) {
        if let Some(id) = self.doc {
            self.scrolls.insert(id, y);
        }
        if let Some(path) = self.doc_path.as_ref() {
            if y > 0.0 {
                if self.path_scrolls.len() >= RECENT_DOC_STATES
                    && !self.path_scrolls.contains_key(path)
                {
                    self.path_scrolls.clear();
                }
                self.path_scrolls.insert(path.clone(), y);
            } else {
                self.path_scrolls.remove(path);
            }
        }
    }

    /// 丢弃已读取的图片（联网设置改变时调用）：下次绘制时重新排版并重新读取。
    pub fn forget_images(&mut self) {
        self.images.forget();
    }

    /// 解析结果是否与文档版本 `revision` 一致。
    pub(crate) fn is_checked(&self, revision: u64) -> bool {
        self.checked == Some(revision)
    }

    /// 将预览切换为文档 `doc`（不再显示预览时为 `None`）。
    ///
    /// 释放上一个文档的解析结果与排版，但保留按路径记忆的滚动位置、属性折叠态、最近几篇的排版缓存
    /// （Mermaid/公式/代码高亮）以及有界数量的本地渲染图片，切回同一笔记时可首帧直接复用。
    fn reset(&mut self, doc: Option<DocumentId>, path: Option<PathBuf>) {
        self.leave_book();
        if let Some(old_path) = self.doc_path.take() {
            if let Some(old_id) = self.doc
                && let Some(&y) = self.scrolls.get(&old_id)
            {
                if y > 0.0 {
                    if self.path_scrolls.len() >= RECENT_DOC_STATES
                        && !self.path_scrolls.contains_key(&old_path)
                    {
                        self.path_scrolls.clear();
                    }
                    self.path_scrolls.insert(old_path.clone(), y);
                } else {
                    self.path_scrolls.remove(&old_path);
                }
            }
            let old_memo = std::mem::take(&mut self.memo);
            self.recent_memos.retain(|(p, _)| p != &old_path);
            self.recent_memos.push_front((old_path, old_memo));
            self.recent_memos.truncate(RECENT_MEMOS);
        } else {
            self.memo = Default::default();
        }
        self.doc = doc;
        self.doc_path = path.clone();
        if let (Some(id), Some(new_path)) = (doc, path.as_ref()) {
            if !self.scrolls.contains_key(&id)
                && let Some(&saved) = self.path_scrolls.get(new_path)
            {
                self.scrolls.insert(id, saved);
            }
            if self.path_property_collapsed.contains(new_path) {
                self.property_collapsed.insert(id);
            }
            if let Some(pos) = self.recent_memos.iter().position(|(p, _)| p == new_path)
                && let Some((_, memo)) = self.recent_memos.remove(pos)
            {
                self.memo = memo;
            }
        }
        self.parsed = None;
        self.parse_serial = self.parse_serial.wrapping_add(1);
        self.checked = None;
        self.pending = None;
        self.first_parse_since = None;
        self.cost = None;
        self.layout = None;
        self.live.reset_doc();
        self.find_doc = None;
        self.motion = None;
        self.images.reset(doc.is_some());
        self.failed = None;
        self.contents = None;
        self.decorations.clear();
        self.decorations_revision = None;
        self.search_deco_key = None;
        self.diag_deco_key = None;
    }
}

/// 搜索高亮与断链诊断层使用的保留 ID 段，避免与用户批注 / AI 挂件标识冲突。
const SEARCH_DECO_BASE: u64 = 0x4000_0000_0000_0000;
const DIAG_DECO_BASE: u64 = 0x8000_0000_0000_0000;

impl App {
    /// 按当前活动文档的最新编辑记录同步装饰层锚点，并刷新查找高亮与断链诊断层。
    pub(crate) fn sync_markdown_decorations(&mut self) {
        let Some(doc) = self.editor.active() else {
            return;
        };
        let (doc_id, doc_rev) = (doc.id, doc.revision);
        if self.preview.markdown.doc != Some(doc_id) {
            return;
        }
        // 1. 先按增量字节变更推进已有的持久装饰（批注、AI 原位预览、块级挂件等）。
        if let Some(prev_rev) = self.preview.markdown.decorations_revision
            && prev_rev != doc_rev
            && !self.preview.markdown.decorations.is_empty()
            && let Some((old_range, new_range)) = doc.byte_edit_since(prev_rev)
        {
            let change =
                winc_richtext::ChangeMap::single(old_range.start, old_range.len(), new_range.len());
            self.preview.markdown.decorations.map(&change);
        }
        self.preview.markdown.decorations_revision = Some(doc_rev);

        // 2. 同步实时编辑预览的查找匹配高亮层（`DecorationLayer::Search`）。
        let search_active = self.markdown_live()
            && self.layout.search.h > 0.0
            && self.bars.search.document() == Some(doc_id);
        if search_active {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let matches = self.bars.search.matches();
            let current = self.bars.search.current_match();
            let mut h = DefaultHasher::new();
            matches.len().hash(&mut h);
            matches.first().hash(&mut h);
            matches.last().hash(&mut h);
            current.hash(&mut h);
            self.palette.search_decoration_fingerprint().hash(&mut h);
            let key = Some((doc_id, doc_rev, h.finish()));
            if self.preview.markdown.search_deco_key != key {
                let mut built = Vec::with_capacity(matches.len());
                if let Some(rope) = self.editor.active().and_then(|d| d.text().rope()) {
                    let len = rope.len_chars();
                    for (idx, &(a, b)) in matches.iter().enumerate() {
                        let start = rope.char_to_byte(a.min(len));
                        let end = rope.char_to_byte(b.min(len));
                        if start < end {
                            let color = self
                                .palette
                                .search_decoration_color(current == Some((a, b)));
                            built.push(winc_richtext::Decoration::mark(
                                winc_richtext::DecorationId(SEARCH_DECO_BASE + idx as u64),
                                winc_richtext::DecorationLayer::Search,
                                start..end,
                                winc_richtext::MarkStyle::Background(color),
                            ));
                        }
                    }
                }
                self.preview
                    .markdown
                    .decorations
                    .replace_layer(winc_richtext::DecorationLayer::Search, built);
                self.preview.markdown.search_deco_key = key;
            }
        } else if self.preview.markdown.search_deco_key.take().is_some() {
            self.preview
                .markdown
                .decorations
                .clear_layer(winc_richtext::DecorationLayer::Search);
        }

        // 3. 同步断链诊断层（`DecorationLayer::Diagnostic`）：仅在解析结果追上当前版本时重算，
        // 异步重解析期间保留上一步 `ChangeMap` 映射后的锚点。
        if self.preview.markdown.checked == Some(doc_rev) && self.preview.markdown.parsed.is_some()
        {
            let diag_key = Some((
                doc_id,
                self.preview.markdown.parse_serial,
                self.preview.markdown.links.generation,
                self.palette.error.r.to_bits(),
            ));
            if self.preview.markdown.diag_deco_key != diag_key
                && let Some(parsed) = self.preview.markdown.parsed.clone()
            {
                let color = self.palette.error.alpha(0.7);
                let mut built = Vec::new();
                for (idx, (range, url)) in parsed.links_with_ranges().into_iter().enumerate() {
                    if url.starts_with('\u{1}') || self.check_markdown_link(url).is_none() {
                        continue;
                    }
                    built.push(winc_richtext::Decoration::mark(
                        winc_richtext::DecorationId(DIAG_DECO_BASE + idx as u64),
                        winc_richtext::DecorationLayer::Diagnostic,
                        range,
                        winc_richtext::MarkStyle::DashedUnderline(color),
                    ));
                }
                self.preview
                    .markdown
                    .decorations
                    .replace_layer(winc_richtext::DecorationLayer::Diagnostic, built);
                self.preview.markdown.diag_deco_key = diag_key;
            }
        }
    }

    /// 返回当前标签显示 Markdown 预览的区域（分栏的右半边或整个编辑区）；不显示时返回 `None`。
    pub(crate) fn markdown_shown_area(&self) -> Option<winc_render::Rect> {
        let doc = self.editor.active()?;
        // docx、EPUB 和内容已取回的阅读标签没有源代码可切换，总是以整个编辑区显示阅读视图。
        let reading = matches!(doc.reading, Some(winc_editor::Reading::Ready(_)));
        let converted =
            (doc.unsupported.as_ref()).is_some_and(winc_editor::Unsupported::reads_as_markdown);
        if converted || reading {
            return Some(self.layout.editor);
        }
        if doc.preview_kind() != Some(PreviewKind::Markdown) {
            return None;
        }
        match doc.preview_mode() {
            PreviewMode::Split if self.layout.split.w > 0.0 => Some(self.layout.split),
            PreviewMode::Preview => Some(self.layout.editor),
            _ => None,
        }
    }

    /// 横向滚动窗口坐标 `(x, y)` 处的可横向滚动表格，返回是否消费了该滚轮事件。
    pub(in crate::app) fn scroll_markdown_table(&mut self, x: f32, y: f32, dx: f32) -> bool {
        if self.preview.markdown.scroll_table(x, y, dx) {
            self.request_redraw();
            return true;
        }
        false
    }

    /// 当前标签已读好且需要显示 Markdown 预览、但还没有第一份解析结果时，立即同步解析（常规文档）
    /// 或启动后台解析并记录起始时刻（供 [`App::holding_frame`] 在首次解析完成前保留上一帧）。
    pub(in crate::app) fn prepare_markdown_after_switch(
        &mut self,
        hold_since: Option<std::time::Instant>,
    ) {
        if self.settings.standalone || self.settings.open || self.library_shown() {
            return;
        }
        if self.editor.active().is_none_or(|d| d.is_pending()) {
            return;
        }
        self.poll_markdown();
        let Some(id) = self.editor.active().map(|d| d.id) else {
            return;
        };
        if self.markdown_shown_area().is_none() {
            return;
        }
        let md = &self.preview.markdown;
        let needs_first = md.doc != Some(id) || (md.parsed.is_none() && md.failed.is_none());
        if needs_first {
            self.advance_markdown_with_since(hold_since);
        }
    }

    /// 当前文档正在等待首次 Markdown 解析完成时，返回开始等待的时刻（供 [`App::holding_frame`] 使用）。
    pub(in crate::app) fn markdown_holding_first_parse(
        &self,
        id: DocumentId,
    ) -> Option<std::time::Instant> {
        if self.settings.standalone || self.settings.open || self.library_shown() {
            return None;
        }
        self.markdown_shown_area()?;
        let md = &self.preview.markdown;
        let (doc, since) = md.first_parse_since?;
        (doc == id
            && md.doc == Some(id)
            && md.parsed.is_none()
            && md.failed.is_none()
            && md.pending.is_some())
        .then_some(since)
    }

    /// 当前标签显示 Markdown 预览且解析结果落后于文字时，在后台解析最新的文字。
    ///
    /// 不再显示预览时释放预览占用的内存，见 [`MarkdownPreview::reset`]。
    ///
    /// 编辑器核心 · 解析调度：每次绘制前检查版本。后台解析读取原文快照，不写回文档；
    /// 输入赶在结果之前时，光标所在块仍可参考当前原文即时排版。
    pub(crate) fn advance_markdown(&mut self) {
        self.advance_markdown_with_since(None);
    }

    fn advance_markdown_with_since(&mut self, hold_since: Option<std::time::Instant>) {
        let shown = self.markdown_shown_area().is_some();
        let Some(doc) = self.editor.active().filter(|_| shown) else {
            if self.preview.markdown.doc.is_some() {
                self.preview.markdown.reset(None, None);
            }
            return;
        };
        let (id, revision, path) = (doc.id, doc.revision, doc.path.clone());
        let md = &mut self.preview.markdown;
        if md.doc != Some(id) {
            md.reset(Some(id), path);
        } else if md.doc_path != path {
            md.doc_path = path;
        }
        if md.checked == Some(revision) {
            return;
        }
        if self.parse_markdown_inline() || self.preview.markdown.pending.is_some() {
            return;
        }
        let Some(doc) = self.editor.active() else {
            return;
        };
        let md = &mut self.preview.markdown;
        if let Some(kind) = doc.unsupported.clone().filter(|u| u.reads_as_markdown()) {
            let Some(path) = doc.path.clone() else {
                return;
            };
            md.pending = Some(revision);
            if md.parsed.is_none() && md.first_parse_since.is_none_or(|(s, _)| s != id) {
                md.first_parse_since = Some((id, hold_since.unwrap_or_else(std::time::Instant::now)));
            }
            let (sender, proxy) = (md.sender.clone(), self.link.proxy.clone());
            winc_editor::background::spawn(move || {
                let result = match kind {
                    winc_editor::Unsupported::Epub => winc_preview::epub::load(&path).map(|b| {
                        let contents = Some(Arc::from(b.contents));
                        (markdown::parse(&b.markdown), b.media, contents)
                    }),
                    _ => winc_preview::docx::load(&path)
                        .map(|c| (markdown::parse(&c.markdown), c.media, None)),
                };
                if sender.send(Done::Converted(id, revision, result)).is_ok()
                    && let Some(proxy) = proxy
                {
                    let _ = proxy.send_event(UserEvent::ImageReady);
                }
            });
            return;
        }
        // 克隆 rope 只增加一个引用；拼接整段文字（16 MB 的文件需要数毫秒）在后台进行。
        let Some(rope) = doc.text().rope().cloned() else {
            return;
        };
        // 阅读标签的原文不在文档的文本中。
        let reading = match &doc.reading {
            Some(winc_editor::Reading::Ready(text)) => Some(Arc::clone(text)),
            _ => None,
        };
        // 非可编辑预览（只读、分栏、阅读标签）在首次切换且体积较小时同样同步解析，
        // 保证当帧即可直接绘制完整内容，不闪出空白帧。
        let bytes = reading
            .as_ref()
            .map_or_else(|| rope.len_bytes(), |text| text.len());
        if md.parsed.is_none() && md.cost.is_none() && bytes <= INLINE_FIRST_PARSE_BYTES {
            let started = std::time::Instant::now();
            let text = reading.map_or_else(|| rope.to_string(), |text| text.to_string());
            let parsed = crate::frame_log::time_parse(|| markdown::parse(&text));
            md.cost = Some(started.elapsed());
            md.pending = None;
            md.first_parse_since = None;
            md.checked = Some(revision);
            md.parsed = Some(Arc::new(parsed));
            md.parse_serial = md.parse_serial.wrapping_add(1);
            return;
        }
        md.pending = Some(revision);
        if md.parsed.is_none() && md.first_parse_since.is_none_or(|(s, _)| s != id) {
            md.first_parse_since = Some((id, hold_since.unwrap_or_else(std::time::Instant::now)));
        }
        let (sender, proxy) = (md.sender.clone(), self.link.proxy.clone());
        winc_editor::background::spawn(move || {
            let started = std::time::Instant::now();
            let text = reading.map_or_else(|| rope.to_string(), |text| text.to_string());
            let parsed = markdown::parse(&text);
            let done = Done::Parsed(id, revision, parsed, started.elapsed());
            if sender.send(done).is_ok()
                && let Some(proxy) = proxy
            {
                let _ = proxy.send_event(UserEvent::ImageReady);
            }
        });
    }

    /// 可编辑的预览中解析结果落后于文字时，在界面线程上解析最新的文字，返回是否已解析。
    ///
    /// 在首次切换且体积不超过 [`INLINE_FIRST_PARSE_BYTES`]，或上一次解析不超过 [`INLINE_PARSE`] 时进行：
    /// 切页、输入、点击、移动光标之后的排版和按键处理都依据当前文字的结构，不显示空白帧或按上一版
    /// 结构推算的中间帧。正在后台解析的旧版本作废。
    pub(crate) fn parse_markdown_inline(&mut self) -> bool {
        if !self.markdown_live() {
            return false;
        }
        let Some(doc) = self.editor.active() else {
            return false;
        };
        let (id, revision) = (doc.id, doc.revision);
        let md = &mut self.preview.markdown;
        if md.doc != Some(id) {
            return false;
        }
        if md.checked == Some(revision) {
            return true;
        }
        let Some(rope) = doc.text().rope() else {
            return false;
        };
        // 优先尝试就地块级局部增量重解析：直接复用未改动的全部顶层块（零深拷贝），
        // 即便整篇文档很长（全文解析耗时超过 `INLINE_PARSE`），常规输入仍可在微秒级同步完成。
        let incremental_ok = crate::frame_log::time_parse(|| {
            if let (Some(prev_arc), Some(prev_rev)) = (md.parsed.as_mut(), md.checked)
                && let Some((old_edit, new_edit)) = doc.byte_edit_since(prev_rev)
            {
                markdown::reparse_local_in_place(
                    Arc::make_mut(prev_arc),
                    old_edit,
                    new_edit,
                    rope.len_bytes(),
                    |b| rope.byte_to_line(b.min(rope.len_bytes())),
                    |r| rope.byte_slice(r).to_string(),
                )
            } else {
                false
            }
        });
        if incremental_ok {
            md.parse_serial = md.parse_serial.wrapping_add(1);
            md.pending = None;
            md.first_parse_since = None;
            md.checked = Some(revision);
            return true;
        }
        let first_small = md.parsed.is_none()
            && md.cost.is_none()
            && rope.len_bytes() <= INLINE_FIRST_PARSE_BYTES;
        if !first_small && md.cost.is_none_or(|c| c > INLINE_PARSE) {
            return false;
        }
        let started = std::time::Instant::now();
        let parsed = crate::frame_log::time_parse(|| markdown::parse(&rope.to_string()));
        md.cost = Some(started.elapsed());
        md.pending = None;
        md.first_parse_since = None;
        md.checked = Some(revision);
        md.parsed = Some(Arc::new(parsed));
        md.parse_serial = md.parse_serial.wrapping_add(1);
        true
    }

    /// 接收后台完成的解析结果和图片。
    pub(crate) fn poll_markdown(&mut self) {
        let mut changed = false;
        let mut converted = false;
        let mut book = None;
        while let Ok(done) = self.preview.markdown.receiver.try_recv() {
            let md = &mut self.preview.markdown;
            match done {
                Done::Parsed(id, revision, parsed, cost) => {
                    if md.doc != Some(id) || md.pending != Some(revision) {
                        continue;
                    }
                    md.cost = Some(cost);
                    md.pending = None;
                    md.first_parse_since = None;
                    md.checked = Some(revision);
                    md.parsed = Some(Arc::new(parsed));
                    md.parse_serial = md.parse_serial.wrapping_add(1);
                }
                Done::Converted(id, revision, result) => {
                    if md.doc != Some(id) || md.pending != Some(revision) {
                        continue;
                    }
                    md.pending = None;
                    md.first_parse_since = None;
                    md.checked = Some(revision);
                    converted = true;
                    md.forget_images();
                    md.images.clear_embedded();
                    match result {
                        Ok((parsed, media, contents)) => {
                            // 本次打开后尚未滚动过的书，排版后回到记下的阅读位置。
                            let path = self.editor.active().and_then(|d| d.path.clone());
                            if contents.is_some() {
                                book = path.map(|p| (p, !md.scrolls.contains_key(&id)));
                            }
                            md.contents = contents;
                            let doc = self.editor.active().and_then(|d| d.path.as_deref());
                            let embedded = media
                                .into_iter()
                                .filter_map(|(url, bytes)| {
                                    Some((image_path(doc, &url, false)?, bytes))
                                })
                                .collect();
                            md.images.replace_embedded(embedded);
                            md.failed = None;
                            md.parsed = Some(Arc::new(parsed));
                            md.parse_serial = md.parse_serial.wrapping_add(1);
                        }
                        Err(reason) => {
                            md.failed = Some(reason);
                            md.parsed = None;
                            md.parse_serial = md.parse_serial.wrapping_add(1);
                        }
                    }
                }
                Done::Loaded(path, result) => {
                    // 读取期间图片已作废（切换了文档或联网设置变化），丢弃结果。
                    if !md.images.finish_load(path, result) {
                        continue;
                    }
                }
                Done::Drawn(path, drawn) => md.images.finish_draw(&path, drawn),
                Done::Undrawn(path, size) => md.images.cancel_draw(&path, size),
            }
            changed = true;
        }
        // EPUB 的目录随转换结果送到，大纲重新列出。
        if converted {
            self.outline_source_changed();
        }
        if let Some((path, fresh)) = book {
            self.begin_book(path, fresh);
        }
        if changed {
            self.request_redraw();
        }
    }

    /// 在编辑器中打开预览里链接指向的本地文件。
    ///
    /// 目标同为 Markdown 时直接显示可编辑的排版视图；`其他.md#标题` 在排版完成后滚到该标题。
    pub(in crate::app) fn open_markdown_file(&mut self, path: &Path, url: &str) {
        let mode = self
            .editor
            .active()
            .map(winc_editor::Document::preview_mode);
        let anchor = url
            .split_once('#')
            .map(|(_, anchor)| anchor.to_owned())
            .filter(|a| !a.is_empty());
        self.open_path_then(path, move |app| {
            let Some(doc) = app.editor.active() else {
                return;
            };
            let id = doc.id;
            match doc.preview_kind() {
                Some(PreviewKind::Markdown) if doc.preview_mode() != PreviewMode::Preview => {
                    app.set_preview_mode(PreviewMode::Preview);
                }
                Some(PreviewKind::Svg | PreviewKind::Table | PreviewKind::Json)
                    if doc.preview_mode() == PreviewMode::Source =>
                {
                    if let Some(mode) = mode {
                        app.set_preview_mode(mode);
                    }
                }
                _ => {}
            }
            if let Some(anchor) = anchor {
                app.preview.markdown.pending_anchor = Some((id, anchor));
            }
        });
    }

    /// 键盘焦点在预览上时（只看预览，或分栏时点击了右侧）按键滚动预览，返回是否处理了该键。
    ///
    /// ↑↓ 滚动一小段，`PageUp` / `PageDown`、空格（加 ⇧ 向上）滚动一屏，Home / End、
    /// ⌘↑ / ⌘↓ 滚动到开头、末尾；Esc 清除选区。分栏时在预览上输入、回车、删除：焦点回到
    /// 左侧的文字，按普通编辑处理。可编辑的预览中按键作用于文档，见 [`App::live_key`]。
    pub(in crate::app) fn markdown_key(&mut self, key: &Key, cmd: bool) -> bool {
        let Some(area) = self.markdown_shown_area() else {
            return false;
        };
        if self.markdown_live() {
            return self.live_key(key, cmd, area);
        }
        if !self.markdown_focused() {
            return false;
        }
        if *key == Key::Named(NamedKey::Escape) {
            if self.preview.markdown.selection.take().is_some() {
                self.request_redraw();
                return true;
            }
            return false;
        }
        let split = area == self.layout.split;
        let typing = matches!(
            key,
            Key::Character(c) if c != " "
        ) || matches!(
            key,
            Key::Named(NamedKey::Enter | NamedKey::Backspace | NamedKey::Delete | NamedKey::Tab)
        );
        if split && typing {
            self.preview.markdown.focused = false;
            self.preview.markdown.selection = None;
            return false;
        }
        let step = self.fonts.line_height * 2.0;
        let page = (area.h * 0.9).max(step);
        let back = self.pointer.modifiers.shift_key();
        let y = self.preview.markdown.scroll();
        // 方向键逐段滚动，直接到达；翻页和到两端的跳转平滑滚动。
        let small = matches!(key, Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown)) && !cmd;
        let to = match key {
            Key::Named(NamedKey::ArrowUp) if cmd => 0.0,
            Key::Named(NamedKey::ArrowDown) if cmd => f32::MAX,
            Key::Named(NamedKey::ArrowUp) => y - step,
            Key::Named(NamedKey::ArrowDown) => y + step,
            Key::Named(NamedKey::PageUp) => y - page,
            Key::Named(NamedKey::PageDown) => y + page,
            Key::Named(NamedKey::Space) if back => y - page,
            Key::Named(NamedKey::Space) => y + page,
            Key::Character(c) if c == " " && back => y - page,
            Key::Character(c) if c == " " => y + page,
            Key::Named(NamedKey::Home) => 0.0,
            Key::Named(NamedKey::End) => f32::MAX,
            _ => return false,
        };
        if small {
            self.scroll_markdown_to(to);
        } else {
            self.glide_pane_to(ScrollTarget::Markdown, to);
        }
        true
    }

    /// 键盘与查找当前是否作用于预览排版后的文字（选中、复制、查找）。
    ///
    /// 分栏时取决于最近一次点击是否在预览上。只看预览时预览可以直接编辑，键盘作用于文档
    /// （见 [`App::markdown_live`]）；只读文档仍然作用于预览。
    pub(crate) fn markdown_focused(&self) -> bool {
        self.markdown_shown_area().is_some_and(|a| {
            if a == self.layout.editor {
                !self.markdown_live()
            } else {
                self.preview.markdown.focused
            }
        })
    }

    /// Markdown 排版视图默认不显示行号；不影响源码编辑区的设置。
    pub(crate) fn markdown_line_numbers_on(&self) -> bool {
        self.config.markdown_line_numbers == Some(true)
    }

    /// 当前标签是否为可直接编辑的 Markdown 预览（只看预览）。
    pub(crate) fn markdown_live(&self) -> bool {
        self.markdown_shown_area()
            .is_some_and(|a| a == self.layout.editor)
            && self
                .editor
                .active()
                .is_some_and(|d| d.edits_text() && d.readonly_reason().is_none())
    }

    /// 重排后开始过渡动画（见 [`BlockMotion`]）。
    ///
    /// 各块从偏移 `from` 处滑动到新位置，`fading` 中的项淡入。
    ///
    /// 没有需要移动或淡入的内容、或动态效果关闭时不开始动画，直接绘制新的排版。
    pub(crate) fn start_markdown_motion(&mut self, moves: Moves) {
        let Moves {
            from,
            fading,
            inner,
            leaving,
        } = moves;
        let still = from.iter().all(|d| d.abs() < 0.5);
        if still && fading.is_empty() && inner.is_empty() && leaving.is_empty() {
            self.preview.markdown.motion = None;
            return;
        }
        let motion = |clock| BlockMotion {
            clock,
            from,
            fading,
            inner,
            leaving,
        };
        #[cfg(test)]
        if let Some(at) = self.preview.markdown.frozen {
            let clock = crate::app::Clock::frozen(at, crate::app::animation::MARKDOWN_MOTION);
            self.preview.markdown.motion = Some(motion(clock));
            return;
        }
        self.preview.markdown.motion = self
            .clock(crate::app::animation::MARKDOWN_MOTION)
            .map(motion);
    }

    /// 处理预览上的鼠标按下：获取焦点并从该处开始选择文字；双击选中一个词，按住 ⇧ 时扩展选区。
    ///
    /// 可编辑的预览中改为放置文档的光标，见 [`App::click_live`]。
    pub(in crate::app) fn click_markdown(
        &mut self,
        x: f32,
        y: f32,
        native_click_count: Option<u8>,
    ) {
        self.focus = crate::app::Focus::Editor;
        // 内容一栏两侧的空白：分栏时将键盘焦点交给预览，但不放置光标、不开始选择，保留已有选区。
        if !self.preview.markdown.in_content(x) {
            if !self.markdown_live() {
                self.preview.markdown.focused = true;
                self.request_redraw();
            }
            return;
        }
        if self.markdown_live() {
            self.click_live(x, y, native_click_count);
            return;
        }
        self.preview.markdown.focused = true;
        self.request_redraw();
        let Some(at) = self.preview.markdown.offset_at(x, y) else {
            self.preview.markdown.selection = None;
            return;
        };
        let count = self.click_count(x, y);
        let shift = self.pointer.modifiers.shift_key();
        self.preview.markdown.selection = match self.preview.markdown.selection {
            _ if count >= 2 => self.markdown_word(at),
            Some((anchor, _)) if shift => Some((anchor, at)),
            _ => Some((at, at)),
        };
        self.pointer.drag = crate::app::Drag::MarkdownText;
        // 分栏时左侧的光标移到点中的字上：随后输入的文字（输入时焦点回到左侧）插入在此处。只看
        // 预览时光标不可见，不移动。
        if count == 1
            && !shift
            && self
                .markdown_shown_area()
                .is_some_and(|a| a == self.layout.split)
            && let Some(source) = self.current_source_at(x, y)
        {
            self.place_caret_at_source(source);
            // 预览不随左侧滚动：点中的字保持在鼠标下方。
            if let (Some(laid), Some(doc)) =
                (self.preview.markdown.layout.as_ref(), self.editor.active())
            {
                self.preview.markdown.synced = Some((doc.id, doc.scroll_line, laid.height()));
            }
        }
    }

    /// 文字光标在原文中的位置（字节偏移）；解析结果不是当前版本时为 `None`。
    pub(crate) fn markdown_caret_source(&self) -> Option<usize> {
        let doc = self.editor.active()?;
        if self.preview.markdown.checked != Some(doc.revision) {
            return None;
        }
        let rope = doc.text().rope()?;
        Some(rope.char_to_byte(doc.selection.head.min(rope.len_chars())))
    }

    /// 返回预览上 `(x, y)` 处的字在原文中的位置（字节偏移）；解析结果不是当前版本时为 `None`
    /// （刚输入的文字尚未解析，位置可能已错开）。
    fn current_source_at(&self, x: f32, y: f32) -> Option<usize> {
        let doc = self.editor.active()?;
        // docx 的预览由转换得到，位置不对应文档中的文字。
        if doc.unsupported.is_some() || self.preview.markdown.checked != Some(doc.revision) {
            return None;
        }
        self.preview.markdown.source_at(x, y)
    }

    /// 将文字光标放到原文第 `source` 个字节处；光标所在行不可见时滚动到距顶部三分之一处。
    fn place_caret_at_source(&mut self, source: usize) {
        let Some(doc) = self.editor.active_mut() else {
            return;
        };
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let pos = rope.byte_to_char(source.min(rope.len_bytes()));
        doc.goal_cell = None;
        doc.record_cursors();
        doc.collapse_selections();
        doc.selection = winc_core::Selection::point(pos);
        let line = doc.text().pos_to_line_col(pos).0;
        self.scroll_to_line(line, 1.0 / 3.0, true);
        self.request_redraw();
    }

    /// 处理预览上不在链接上的右键点击，返回是否点在预览上。
    ///
    /// 预览获得焦点，点击位置在选区外时取消选区。可编辑的预览中改为将文档的光标移到该处
    /// （点在文档的选区内时不移动）。
    pub(in crate::app) fn markdown_context_target(&mut self, x: f32, y: f32) -> bool {
        let pictures = &self.hits.markdown_pictures;
        let picture = (pictures.iter().rev()).find(|(r, _)| r.contains(x, y));
        self.preview.markdown.menu_picture = picture.map(|(_, path)| path.clone());
        // 链接上的右键点击不在此处理，包在链接中的图片除外。
        let on_link = self.markdown_link_at(x, y).is_some() && picture.is_none();
        if !self.preview.markdown.area.contains(x, y) || on_link {
            return false;
        }
        self.focus = crate::app::Focus::Editor;
        if self.markdown_live() {
            self.preview.markdown.menu_image = self.image_at(x, y);
            if let Some(source) = self.preview.markdown.source_at(x, y)
                && let Some(doc) = self.editor.active_mut()
                && let Some(rope) = doc.text().rope()
            {
                let pos = rope.byte_to_char(source.min(rope.len_bytes()));
                if !(doc.selection.from()..=doc.selection.to()).contains(&pos) {
                    doc.selection = winc_core::Selection::point(pos);
                    doc.goal_cell = None;
                }
            }
            self.request_redraw();
            return true;
        }
        self.preview.markdown.focused = true;
        let at = self.preview.markdown.offset_at(x, y);
        let inside = self
            .preview
            .markdown
            .selection
            .zip(at)
            .is_some_and(|((a, b), at)| (a.min(b)..=a.max(b)).contains(&at));
        if !inside {
            self.preview.markdown.selection = None;
        }
        self.request_redraw();
        true
    }

    /// 返回双击处的词：连续的字母数字、连续的中日韩文字，或单个其他字符。
    fn markdown_word(&self, at: usize) -> Option<(usize, usize)> {
        let laid = self.preview.markdown.layout.as_ref()?;
        const REACH: usize = 64;
        let from = at.saturating_sub(REACH);
        let text: Vec<char> = laid
            .text(from, (at + REACH).min(laid.chars()))
            .chars()
            .collect();
        let i = at - from;
        let class = |c: char| {
            if c.is_alphanumeric() && (c as u32) < 0x2E80 || c == '_' {
                1
            } else if c.is_alphanumeric() {
                2
            } else {
                0
            }
        };
        let kind = class(*text.get(i)?);
        if kind == 0 {
            return Some((at, at + 1));
        }
        let start = text[..i]
            .iter()
            .rposition(|&c| class(c) != kind)
            .map_or(0, |p| p + 1);
        let end = text[i..]
            .iter()
            .position(|&c| class(c) != kind)
            .map_or(text.len(), |p| i + p);
        Some((from + start, from + end))
    }

    /// 处理拖动选择文字：拖出上下边缘时随之滚动。
    pub(in crate::app) fn drag_markdown(&mut self, x: f32, y: f32) {
        let area = self.preview.markdown.area;
        if y < area.y {
            self.scroll_markdown((area.y - y).min(60.0));
        } else if y > area.bottom() {
            self.scroll_markdown(-(y - area.bottom()).min(60.0));
        }
        let y = y.clamp(area.y, (area.bottom() - 1.0).max(area.y));
        if self.markdown_live() {
            self.drag_live(x, y);
            self.request_redraw();
            return;
        }
        if let Some(at) = self.preview.markdown.offset_at(x, y)
            && let Some((anchor, _)) = self.preview.markdown.selection
        {
            self.preview.markdown.selection = Some((anchor, at));
        }
        self.request_redraw();
    }

    /// 返回预览中选中的文字；没有选中时返回 `None`。
    pub(crate) fn markdown_selected_text(&self) -> Option<String> {
        let (a, b) = self.preview.markdown.selection?;
        let text = self
            .preview
            .markdown
            .layout
            .as_ref()?
            .text(a.min(b), a.max(b));
        (!text.is_empty()).then_some(text)
    }

    /// 选中预览中的全部文字（⌘A）。
    pub(crate) fn markdown_select_all(&mut self) {
        let chars = self
            .preview
            .markdown
            .layout
            .as_ref()
            .map_or(0, |l| l.chars());
        self.preview.markdown.selection = Some((0, chars));
        self.request_redraw();
    }

    /// 返回查找用的预览文字（见 [`MarkdownPreview::find_doc`]），尚未装入时立即装入。
    ///
    /// 尚未排版时返回 `None`。
    pub(crate) fn markdown_find_doc(&mut self) -> Option<&winc_editor::Document> {
        if self.preview.markdown.find_doc.is_none() {
            let text = self.preview.markdown.full_text()?;
            let md = &mut self.preview.markdown;
            md.find_serial += 1;
            // 与实际文档、差异所用的标识互不重叠。
            let id = DocumentId(u64::MAX - 1 - md.find_serial);
            md.find_doc = Some(winc_editor::Document::from_text(id, &text));
        }
        self.preview.markdown.find_doc.as_ref()
    }

    /// 选中预览中第 `from` 到 `to` 个字符（查找跳转到的匹配）并滚动使其可见：已在屏幕内时
    /// 不滚动，否则滚动到距顶部三分之一屏处。
    pub(crate) fn select_in_markdown(&mut self, from: usize, to: usize) {
        self.preview.markdown.selection = Some((from, to));
        if let Some(y) = self.preview.markdown.offset_top(from) {
            let md = &mut self.preview.markdown;
            let scroll = md.scroll();
            if y < scroll || y + 40.0 > scroll + md.area.h {
                let y = y - md.area.h / 3.0;
                self.glide_pane_to(ScrollTarget::Markdown, y);
            }
        }
        self.request_redraw();
    }

    /// 在切换显示方式之前调用：返回只看预览时预览顶部对应的原文行号（小数）。
    ///
    /// 切换为源代码或分栏后，文字滚动到该行（[`App::scroll_text_to`]）；不是只看预览或尚未
    /// 排版时为 `None`。切换为只看预览时，记录在排版完成后对齐一次（见
    /// [`App::align_markdown_once`]）；光标所在的块在预览上开始编辑后才显示原文。
    pub(in crate::app) fn leave_markdown_mode(&mut self, mode: PreviewMode) -> Option<f32> {
        let doc = self.editor.active()?;
        let old = doc.preview_mode();
        self.preview.markdown.live.leave_mode();
        if old == mode || doc.preview_kind() != Some(PreviewKind::Markdown) {
            return None;
        }
        if mode == PreviewMode::Preview {
            self.preview.markdown.align = Some(doc.id);
            return None;
        }
        let laid = self.preview.markdown.layout.as_ref()?;
        (old == PreviewMode::Preview && self.preview.markdown.doc == Some(doc.id))
            .then(|| laid.line_of_y(self.preview.markdown.scroll(), doc.text().line_count()))
    }
}

/// 返回文字滚动位置的第一个显示行 `doc.scroll_line` 对应的原文行号（小数：自动换行折成
/// 多段的行按段数等分）。
fn text_line(doc: &winc_editor::Document) -> f32 {
    let row = doc.scroll_line.max(0.0);
    let (line, part) = doc.line_and_part(row as usize);
    let rows = doc.rows_of(line).max(1) as f64;
    line as f32 + ((part as f64 + row.fract()) / rows) as f32
}

/// [`text_line`] 的逆运算：返回原文第 `line` 行（小数）对应的显示行。
fn text_row(doc: &winc_editor::Document, line: f32) -> f64 {
    let last = doc.text().line_count().saturating_sub(1);
    let whole = (line.max(0.0) as usize).min(last);
    let rows = doc.rows_of(whole).max(1) as f64;
    doc.row_of(whole) as f64 + f64::from((line - whole as f32).clamp(0.0, 1.0)) * rows
}

/// 网址图片的下载缓存目录，文件名为网址的哈希。
const REMOTE_CACHE: &str = "winc-markdown-images";

/// 下载一张图片的最长等待时间（秒），传给 `curl` 的 `--max-time`。
const REMOTE_TIMEOUT: &str = "20";

/// 单张图片的最大下载大小（字节，20 MB），传给 `curl` 的 `--max-filesize`。
const REMOTE_MAX: &str = "20000000";

/// `url` 是否为需要联网的地址。
fn is_remote(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// 返回文档 `doc`（未保存过的文档为 `None`）中图片地址 `url` 对应的文件：本地图片见
/// [`local_target`]；网址的图片为下载的缓存文件，`remote` 为假（不允许联网）时返回 `None`。
pub(crate) fn image_path(doc: Option<&Path>, url: &str, remote: bool) -> Option<PathBuf> {
    if is_remote(url) {
        // FNV-1a：同一网址在每次启动时都映射到同一个文件，已下载的图片无需重新下载。
        let hash = url.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        // 扩展名统一按位图写，读取时按内容判断真实格式。
        return remote.then(|| {
            std::env::temp_dir()
                .join(REMOTE_CACHE)
                .join(format!("{hash:016x}.png"))
        });
    }
    local_target(doc?, url)
}

/// 缓存图片的最长未使用时间（30 天），超过时删除。
pub(in crate::app) const CACHE_AGE: std::time::Duration =
    std::time::Duration::from_secs(30 * 24 * 3600);

/// 缓存目录占用的磁盘上限（字节），200 MB。
const CACHE_MAX: u64 = 200 * 1024 * 1024;

/// 把 `url` 下载到 `path`。
///
/// 已下载过时只更新修改时间（清理缓存时据此判断最久未使用的文件）。先写入同目录的临时
/// 文件，下载完成后再改名，中途失败不会留下不完整的文件。每次启动后首次下载前清理一次缓存
/// （[`clean_cache`]）。
#[allow(clippy::disallowed_methods, reason = "只访问本机：图片缓存在本机")]
fn download(url: &str, path: &Path) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or(t!("preview-markdown-cache-folder-invalid"))?;
    static CLEANED: std::sync::Once = std::sync::Once::new();
    CLEANED.call_once(|| clean_cache(dir, std::time::SystemTime::now()));
    if path.is_file() {
        if let Ok(file) = std::fs::File::options().write(true).open(path) {
            let _ = file.set_modified(std::time::SystemTime::now());
        }
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = path.with_extension(format!("part{}", std::process::id()));
    let mut command = crate::web::curl();
    #[cfg(test)]
    if url.starts_with("http://127.0.0.1:") {
        command.args(["--noproxy", "127.0.0.1"]);
    }
    command
        .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
        .args([
            "-fsSL",
            "--globoff",
            "--max-time",
            REMOTE_TIMEOUT,
            "--max-filesize",
            REMOTE_MAX,
            "-o",
        ])
        .arg(&part)
        .args(["--", url])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = command.status().map_err(|e| e.to_string())?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(t!("preview-markdown-download-failed").to_owned());
    }
    winc_config::files::replace_file(&part, path).map_err(|e| e.to_string())
}

/// 删除 `dir` 中超过 [`CACHE_AGE`] 未使用的图片，以及超过一小时未下载完成的临时文件；
/// 剩余文件超过 [`CACHE_MAX`] 时从最久未使用的开始删除。
#[allow(clippy::disallowed_methods, reason = "只访问本机：图片缓存在本机")]
pub(in crate::app) fn clean_cache(dir: &Path, now: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut kept = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let path = entry.path();
        let age = meta
            .modified()
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .unwrap_or_default();
        let partial = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.starts_with("part"));
        let stale = if partial {
            age > std::time::Duration::from_secs(3600)
        } else {
            age > CACHE_AGE
        };
        if stale {
            let _ = std::fs::remove_file(&path);
        } else if !partial {
            kept.push((age, meta.len(), path));
        }
    }
    let mut total: u64 = kept.iter().map(|k| k.1).sum();
    // 最久未使用的排在前面。
    kept.sort_by_key(|k| std::cmp::Reverse(k.0));
    for (_, len, path) in kept {
        if total <= CACHE_MAX {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// 读取下载的图片：内容为 SVG（徽章多为 SVG）时按 SVG 读取，否则按位图读取。
///
/// SVG 中的文字（徽章上的字）用系统字体绘制，与直接预览 SVG 文件时相同。
#[allow(clippy::disallowed_methods, reason = "只访问本机：图片缓存在本机")]
fn load_downloaded(path: &Path) -> Result<Picture, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let head = &bytes[..bytes.len().min(1024)];
    if head.windows(4).any(|w| w == b"<svg") {
        let text = String::from_utf8_lossy(&bytes);
        return image::load_svg(&text, None, Some(system_glyphs()));
    }
    image::load(path)
}

/// 返回文档 `doc` 中地址 `url` 指向的本地文件；网址、`mailto:` 等返回 `None`。
///
/// 去掉 `#…`、`?…`，相对路径按文档所在目录解析，并还原 `%20` 等转义。`file://主机/路径`
/// 取其中的路径：本地文件都在编辑区中打开，不交给系统，因为系统打开 `.app`、`.command`
/// 时会运行这些文件。
pub(crate) fn local_target(doc: &Path, url: &str) -> Option<PathBuf> {
    let url = match url.strip_prefix("file://") {
        Some(rest) => &rest[rest.find('/')?..],
        None if url.is_empty()
            || url.contains("://")
            || winc_preview::note_links::is_web(url)
            || url.starts_with("mailto:")
            || url.starts_with("data:") =>
        {
            return None;
        }
        None => url,
    };
    let path = url.split(['#', '?']).next().unwrap_or_default();
    if path.is_empty() {
        return None;
    }
    let path = percent_decode(path);
    let path = Path::new(&path);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        doc.parent()?.join(path)
    })
}

/// 还原网址里 `%xx` 形式的转义；不是合法 UTF-8 时原样返回。
pub(crate) fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            )
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

impl App {
    /// 当前标签是否在编辑 Markdown：可以修改的 Markdown 文件，显示着源代码，或者是可编辑的预览。
    pub(crate) fn markdown_writing(&self) -> bool {
        self.markdown_live()
            || self.editor.active().is_some_and(|d| {
                d.preview_kind() == Some(PreviewKind::Markdown)
                    && d.edits_text()
                    && d.readonly_reason().is_none()
                    && d.preview_mode() != PreviewMode::Preview
            })
    }

    /// 是否淡化光标所在段落以外的文字（专注模式，`markdown.focusMode`）。
    pub(crate) fn markdown_focus(&self) -> bool {
        self.config.markdown_focus == Some(true) && self.markdown_writing()
    }

    /// 是否让光标所在的行停在编辑区中间（打字机模式，`markdown.typewriterMode`）。
    pub(crate) fn markdown_typewriter(&self) -> bool {
        self.config.markdown_typewriter == Some(true) && self.markdown_writing()
    }

    /// 编辑区是否使用写作命令的快捷键：可编辑的预览中总是使用，Markdown 文件的源代码在
    /// `markdown.shortcuts` 开着时使用。
    pub(crate) fn markdown_keys(&self) -> bool {
        self.markdown_live()
            || (self.config.markdown_shortcuts != Some(false) && self.markdown_writing())
    }
}

#[cfg(test)]
mod tests;
