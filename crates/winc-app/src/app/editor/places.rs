//! 位置浮窗：在挂在当前光标下的浮窗中列出文档中的书签，支持输入筛选与回车跳转。

use winc_core::movement;
use winc_i18n::t;

use super::*;

/// 一处跳转位置（书签）。
#[derive(Debug, Clone)]
pub struct Place {
    /// 相对于已打开目录的路径，在列表中显示。
    pub relative: String,
    /// 行号（从 0 起）。
    pub line: usize,
    /// 该行的内容（去掉行首空白）。
    pub preview: String,
    /// 高亮范围（字符偏移）；书签为空。
    pub marks: Vec<std::ops::Range<usize>>,
}

/// 卡片中列出的内容种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacesKind {
    /// 一个文件中的书签。
    Bookmarks,
}

impl PlacesKind {
    /// 返回卡片右上角显示的数量文字，`n` 为条目数。
    pub fn count(self, n: usize) -> String {
        match self {
            PlacesKind::Bookmarks => t!("editor-places-bookmarks-count", count = n),
        }
    }
}

/// 在位置浮窗中列出的多处位置（书签）。
#[derive(Debug, Clone)]
pub struct Places {
    /// 列出的内容种类。
    pub kind: PlacesKind,
    /// 列出的各处位置。
    pub items: Vec<Place>,
    /// 当前文件相对于已打开目录的路径，显示在标题行。
    pub origin: String,
    /// 浮窗挂在当前文档的哪个位置（字符偏移）：光标所在词的开头。
    pub anchor: usize,
    /// 全部条目中最宽的「文件名 + 行号」：（界面字号、小字号、宽度）。
    widest_label: std::cell::Cell<Option<(f32, f32, f32)>>,
}

impl Places {
    /// 返回全部条目中最宽的「文件名 + 行号」的宽度（逻辑像素），由 `measure` 量出。
    pub fn widest_label(&self, sizes: (f32, f32), measure: impl FnOnce(&[Place]) -> f32) -> f32 {
        match self.widest_label.get() {
            Some((ui, small, width)) if (ui, small) == sizes => width,
            _ => {
                let width = measure(&self.items);
                self.widest_label.set(Some((sizes.0, sizes.1, width)));
                width
            }
        }
    }
}

impl App {
    /// 跳转到当前文档的第 `line` 行、第 `col` 列，选中长度为 `len` 的文字。
    ///
    /// 行号和列号都从 0 起。当前标签刚打开、还在读时，焦点马上给编辑区，读好再跳
    /// （[`App::when_loaded`]）。
    pub(in crate::app) fn jump_to(&mut self, line: usize, col: usize, len: usize) {
        if self.editor.active().is_none() {
            return;
        }
        self.focus = Focus::Editor;
        self.when_loaded(move |app| app.go_to(line, col, len));
    }

    /// 在挂在光标下方的浮窗中列出 `items`，直接打字可以筛选。
    pub(super) fn show_places(&mut self, kind: PlacesKind, items: Vec<Place>) {
        let Some(doc) = self.editor.active() else {
            return;
        };
        let anchor = movement::word_at(doc.text(), doc.selection.head).0;
        let origin = doc
            .path
            .as_deref()
            .and_then(|path| self.relative_path(path))
            .unwrap_or_else(|| doc.name().into_owned());
        self.finish_name_input();
        self.overlays.picker.places_out = None;
        self.overlays.picker.places_wheel = 0.0;
        self.overlays.picker.places_in = self.clock(POPUP_IN);
        self.overlays.picker.branch_rows = None;
        self.overlays.picker.location_rows = None;
        self.overlays.picker.folders = false;
        self.overlays.picker.languages = false;
        self.overlays.picker.vim_list = None;
        self.overlays.picker.places = Some(Places {
            kind,
            items,
            origin,
            anchor,
            widest_label: Default::default(),
        });
        self.overlays.picker.open = true;
        self.overlays.picker.doc = self.editor.active().map(|d| d.id);
        self.overlays.picker.input.set_text("");
        self.focus = Focus::Picker;
        self.picker_refresh();
        self.relayout();
        self.reset_blink();
    }

    /// 按输入框的文字筛选列表：路径或所在行的内容包含输入的文字（不区分大小写）即保留。
    pub(in crate::app) fn places_refresh(&mut self) {
        let query = self.overlays.picker.input.text().trim().to_lowercase();
        let Some(places) = &self.overlays.picker.places else {
            return;
        };
        self.overlays.picker.hits = places
            .items
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                query.is_empty()
                    || p.relative.to_lowercase().contains(&query)
                    || p.preview.to_lowercase().contains(&query)
            })
            .map(|(i, _)| i)
            .collect();
    }

    /// 处理列表中的回车：关闭面板并跳转到所选的一处。
    pub(in crate::app) fn places_accept(&mut self) {
        let Some(&i) = self.overlays.picker.hits.get(self.overlays.picker.selected) else {
            return;
        };
        let Some(mut places) = self.overlays.picker.places.take_if(|p| i < p.items.len()) else {
            return;
        };
        let place = places.items.swap_remove(i);
        self.close_picker();
        self.jump_to(place.line, 0, 0);
    }
}
