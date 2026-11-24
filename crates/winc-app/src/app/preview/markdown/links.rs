//! 预览中的链接：查找鼠标下的链接，点击时打开网址、跳到标题、打开本地文件，或者执行预览自己
//! 的控件（复制代码、开合折叠块、收起标题、勾选任务）。

use super::*;
use crate::app::Hover;
use crate::app::animation::LinkTooltip;
use crate::app::workspace::note_links::{Wiki, WikiResult, wiki_target};
use winc_preview::note_links::WIKI_TARGET;

/// 正文链接无法打开的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Broken {
    /// 指向的本地文件不存在。
    File,
    /// 文中没有 `#锚点` 对应的标题。
    Heading,
    /// 多篇笔记同名，需要带目录的链接。
    Ambiguous,
}

impl Broken {
    /// 返回悬停提示中显示的原因。
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Broken::File => t!("preview-markdown-link-file-not-found"),
            Broken::Heading => t!("preview-markdown-heading-not-found"),
            Broken::Ambiguous => t!("preview-markdown-link-ambiguous"),
        }
    }
}

/// 已检查的链接：所属文档、版本号、解析的版本号和各地址的结果。
#[derive(Debug, Default)]
pub(crate) struct LinkChecks {
    of: Option<(DocumentId, u64, Option<u64>)>,
    results: HashMap<String, Option<Broken>>,
}

impl LinkChecks {
    /// 返回已检查的地址 `url` 的结果。
    pub(crate) fn get(&self, url: &str) -> Option<Broken> {
        self.results.get(url).copied().flatten()
    }

    /// 清空结果，磁盘上的文件变化后调用。
    pub(crate) fn clear(&mut self) {
        self.results.clear();
    }
}

impl App {
    /// 返回正文链接 `url` 无法打开的原因，结果按文档版本号缓存；网址、脚注和远端文件不检查。
    pub(crate) fn check_markdown_link(&mut self, url: &str) -> Option<Broken> {
        let doc = self.editor.active()?;
        let of = Some((doc.id, doc.revision, self.preview.markdown.checked));
        if self.preview.markdown.links.of != of {
            self.preview.markdown.links = LinkChecks {
                of,
                results: HashMap::new(),
            };
        }
        if let Some(&known) = self.preview.markdown.links.results.get(url) {
            return known;
        }
        let wiki = url.strip_prefix(WIKI_TARGET).map(wiki_target);
        let heading = match wiki {
            Some(Wiki::Heading(heading)) => Some(winc_preview::markdown::slug(heading)),
            _ => None,
        };
        let broken = match heading.as_deref().or_else(|| url.strip_prefix('#')) {
            Some(anchor) if anchor.starts_with("fn-") || anchor.starts_with("fnref-") => None,
            // 收起的一节中的块没有排版，同时按解析结果中的标题查找。
            Some(anchor) => {
                let key = percent_decode(anchor).to_lowercase();
                let parsed = self.preview.markdown.parsed.as_ref();
                let titled = parsed.is_some_and(|p| p.heading_keys().contains(&Some(key)));
                (laid::anchor_box(self, anchor).is_none() && !titled).then_some(Broken::Heading)
            }
            None => match wiki {
                Some(Wiki::Note(name, _)) => match self.wiki_note(name) {
                    Some(WikiResult::Missing) => Some(Broken::File),
                    Some(WikiResult::Ambiguous(_)) => Some(Broken::Ambiguous),
                    _ => None,
                },
                Some(_) => Some(Broken::File),
                None => (doc.path.as_deref())
                    .and_then(|path| local_target(path, url))
                    .filter(|target| !self.may_exist(target))
                    .map(|_| Broken::File),
            },
        };
        (self.preview.markdown.links.results).insert(url.to_owned(), broken);
        broken
    }

    /// 返回预览上 `(x, y)` 处的链接序号（`hits.markdown_links` 中的下标）。
    pub(crate) fn markdown_link_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.preview.markdown.area.contains(x, y) {
            return None;
        }
        self.hits
            .markdown_links
            .iter()
            .position(|(rect, _)| rect.contains(x, y))
    }

    /// 第 `index` 个链接是否为预览自身的控件（代码块的复制按钮、图的全屏按钮、折叠块的标题、
    /// 勾选框），而非正文中的链接。
    pub(crate) fn markdown_link_is_control(&self, index: usize) -> bool {
        use crate::app::preview::markdown::laid::{COPY, DETAILS, EXPAND, TABLE, TASK};
        self.hits.markdown_links.get(index).is_some_and(|(_, url)| {
            [
                COPY,
                DETAILS,
                EXPAND,
                TABLE,
                TASK,
                winc_preview::markdown::properties::PROPERTY,
                crate::app::workspace::note_links::footer::CONTROL,
            ]
            .iter()
            .any(|prefix| url.starts_with(prefix))
        })
    }

    /// 处理预览中链接的点击：网址交给浏览器，`#标题` 滚动到该标题，本地文件在编辑器中打开。
    pub(crate) fn open_markdown_link(&mut self, index: usize) {
        let Some((rect, url)) = self.hits.markdown_links.get(index).cloned() else {
            return;
        };
        if self.press_note_footer(&url) {
            return;
        }
        if url == crate::app::preview::markdown::laid::EXPAND {
            self.open_diagram_view();
            return;
        }
        if url == crate::app::preview::markdown::laid::COPY {
            let Some(code) = self.hits.markdown_copy.clone() else {
                return;
            };
            self.set_clipboard(code.to_string(), false);
            self.flash(t!("preview-markdown-code-copied"));
            return;
        }
        if let Some(property) = url.strip_prefix(winc_preview::markdown::properties::PROPERTY) {
            use super::properties::popup::PropertyInput;
            match property {
                "add" => self.open_property_menu(None, self.pointer.mouse),
                "collapse" => self.toggle_property_panel(),
                "fields" => self.show_property_source(false, rect),
                "source" => self.show_property_source(true, rect),
                "raw" => self.open_property_source(rect),
                _ => {
                    let (action, index) = property.split_once(':').unwrap_or(("", property));
                    if action == "item" {
                        if let Some((row, item)) = index.split_once(':')
                            && let (Ok(row), Ok(item)) = (row.parse(), item.parse())
                        {
                            self.remove_property_item(row, item);
                        }
                    } else if let Ok(index) = index.parse() {
                        match action {
                            "type" => self.open_property_menu(Some(index), self.pointer.mouse),
                            "name" => self.open_property_value(index, rect, PropertyInput::Name),
                            "list" => {
                                self.open_property_value(index, rect, PropertyInput::ListItem)
                            }
                            "delete" => self.remove_note_property(index),
                            _ => self.open_property_value(index, rect, PropertyInput::Value),
                        }
                    }
                }
            }
            return;
        }
        if let Some(op) = url.strip_prefix(crate::app::preview::markdown::laid::TABLE) {
            self.live_table_tool(op);
            return;
        }
        if let Some(at) = url.strip_prefix(crate::app::preview::markdown::laid::TASK) {
            if let Ok(at) = at.parse() {
                self.toggle_markdown_task(at);
            }
            return;
        }
        if let Some(id) = url.strip_prefix(crate::app::preview::markdown::laid::DETAILS) {
            if let Ok(id) = u64::from_str_radix(id, 16)
                && !self.preview.markdown.toggled.remove(&id)
            {
                self.preview.markdown.toggled.insert(id);
            }
            self.preview.markdown.toggled_now = Some(url);
            return;
        }
        self.open_markdown_link_target(&url);
    }

    /// Follow the tooltip's original link even when the card extends into another pane.
    pub(in crate::app) fn open_link_tooltip(&mut self, tip: LinkTooltip) {
        self.set_hover(Hover::None);
        self.focus_pane(tip.pane);
        if self.editor.panes.focused == tip.pane
            && self
                .editor
                .active()
                .is_some_and(|doc| (doc.id, doc.revision) == tip.source)
        {
            self.open_markdown_link_target(&tip.target);
        }
    }

    fn open_markdown_link_target(&mut self, url: &str) {
        if let Some(target) = url.strip_prefix(WIKI_TARGET) {
            self.open_wiki(target);
            return;
        }
        if let Some(anchor) = url.strip_prefix('#') {
            self.jump_to_anchor(anchor);
            return;
        }
        let local = self
            .editor
            .active()
            .and_then(|d| d.path.as_deref())
            .and_then(|path| local_target(path, url));
        match local {
            Some(path) if self.may_be_file(&path) => self.open_markdown_file(&path, url),
            Some(_) => self.flash(t!("preview-markdown-link-file-not-found")),
            None => {
                if let Err(e) = system::open_url(url) {
                    self.flash(t!(
                        "preview-markdown-link-open-failed",
                        error = e.to_string()
                    ));
                }
            }
        }
    }

    fn wiki_note(&self, name: &str) -> Option<WikiResult<'_>> {
        let path = self.editor.active()?.path.as_deref()?;
        let root = self.library_root()?;
        let index = self.note_links.index.as_ref()?;
        Some(index.resolve_wiki(root, path, name))
    }

    fn open_wiki(&mut self, target: &str) {
        match wiki_target(target) {
            Wiki::Heading(heading) => self.jump_to_anchor(&winc_preview::markdown::slug(heading)),
            Wiki::Note(name, heading) => {
                let anchor =
                    heading.map(|heading| format!("#{}", winc_preview::markdown::slug(heading)));
                match self.wiki_note(name) {
                    Some(WikiResult::Found(path)) => {
                        let path = path.to_path_buf();
                        self.open_markdown_file(&path, anchor.as_deref().unwrap_or_default());
                    }
                    Some(WikiResult::Ambiguous(paths)) => {
                        let paths = paths.to_vec();
                        self.choose_wiki_note(&paths, anchor);
                    }
                    None if self.library_root().is_some() && self.note_links.index.is_none() => {
                        self.ensure_note_links();
                        self.flash(t!("workspace-note-links-loading"));
                    }
                    Some(WikiResult::Missing) | None => self.flash(Broken::File.reason()),
                }
            }
            Wiki::Missing => self.flash(Broken::File.reason()),
        }
    }

    /// 编辑器核心 · 最小改写示例：点击勾选框只替换原文 `[ ]` / `[x]` 中间的一个字符，
    /// 不重新生成列表或文档。`at` 是解析时记下的原文字节偏移。
    ///
    /// 修改可以撤销；光标和选区不变，可编辑的预览也不因此进入编辑状态。
    /// 该处不是勾选框（解析结果落后于文字）时不修改。
    fn toggle_markdown_task(&mut self, at: usize) {
        let Some(doc) = self.editor.active() else {
            return;
        };
        let Some(rope) = doc.text().rope() else {
            return;
        };
        let mark = rope
            .get_byte_slice(at..at.saturating_add(3).min(rope.len_bytes()))
            .map(|s| s.to_string());
        let Some(mark) = mark else {
            return;
        };
        let next = match mark.as_str() {
            "[ ]" => "x",
            "[x]" | "[X]" => " ",
            _ => return,
        };
        let (id, pos) = (doc.id, rope.byte_to_char(at) + 1);
        let (selections, primary) = doc.selections();
        let editing = self.preview.markdown.editing == Some(id);
        if let Some(doc) = self.editor.active_mut() {
            doc.collapse_selections();
            doc.selection = winc_core::Selection::new(pos, pos + 1);
        }
        self.run_source_edit(winc_editor::Command::Insert(next.to_owned()));
        if let Some(doc) = self.editor.active_mut() {
            // 替换的只有一个字符，其前后的位置均不变。
            doc.set_selections(selections, primary);
        }
        if !editing {
            self.preview.markdown.editing = None;
            self.preview.markdown.followed = self
                .live_caret_source()
                .zip(self.editor.active())
                .map(|(source, d)| (d.id, source, d.revision));
        }
        self.request_redraw();
    }
}
