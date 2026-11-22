//! JSON 结构图：随窗格保存解析结果、折叠状态和画布位置。

use std::collections::HashSet;
use std::sync::mpsc;

use winc_editor::{DocumentId, PreviewKind, PreviewMode};
use winc_render::Rect;
use winit::event::MouseScrollDelta;

use crate::app::{App, Drag, Focus, UserEvent};
use winc_preview::json::{self, Error, Graph};

mod layout;
pub(crate) use layout::{Layout, ROW_H};

const MIN_ZOOM: f32 = 0.005;
const MAX_ZOOM: f32 = 2.5;

/// 结构图内的点击目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// 尚未生成图，或不是可操作的区域。
    Blank,
    /// 可拖动的画布。
    Canvas,
    /// 适应窗口。
    Fit,
    /// 放大一档。
    ZoomIn,
    /// 缩小一档。
    ZoomOut,
    /// 展开或折叠一行的子卡片。
    Toggle {
        /// 父卡片下标。
        card: usize,
        /// 卡片中的行下标。
        row: usize,
    },
}

type Key = (DocumentId, u64);
type Done = (Key, Result<Graph, Error>);

pub(crate) struct JsonView {
    doc: Option<DocumentId>,
    checked: Option<Key>,
    pending: Option<Key>,
    sender: mpsc::Sender<Done>,
    receiver: mpsc::Receiver<Done>,
    pub(crate) graph: Option<Graph>,
    pub(crate) error: Option<Error>,
    pub(crate) collapsed: HashSet<usize>,
    pub(crate) layout: Option<Layout>,
    pub(crate) area: Rect,
    pub(crate) body: Rect,
    pub(crate) hits: Vec<(Rect, Target)>,
    pub(crate) zoom: f32,
    pub(crate) pan: (f32, f32),
    place: Option<Place>,
}

#[derive(Clone, Copy)]
enum Place {
    Initial,
    Fit,
    KeepRow {
        card: usize,
        row: usize,
        at: (f32, f32),
    },
}

impl Default for JsonView {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            doc: None,
            checked: None,
            pending: None,
            sender,
            receiver,
            graph: None,
            error: None,
            collapsed: HashSet::new(),
            layout: None,
            area: Rect::default(),
            body: Rect::default(),
            hits: Vec::new(),
            zoom: 1.0,
            pan: (0.0, 0.0),
            place: Some(Place::Initial),
        }
    }
}

impl JsonView {
    fn select(&mut self, doc: Option<DocumentId>) {
        if self.doc != doc {
            self.doc = doc;
            self.checked = None;
            self.graph = None;
            self.error = None;
            self.collapsed.clear();
            self.layout = None;
            self.place = Some(Place::Initial);
            self.clear_hits();
        }
    }

    pub(crate) fn clear_hits(&mut self) {
        self.area = Rect::default();
        self.body = Rect::default();
        self.hits.clear();
    }

    fn accept(&mut self, current: Option<Key>, (key, result): Done) -> bool {
        self.pending = None;
        if current != Some(key) || self.doc != Some(key.0) {
            return false;
        }
        self.checked = Some(key);
        match result {
            Ok(graph) => {
                self.graph = Some(graph);
                self.layout = None;
                self.collapsed.clear();
                self.error = None;
                if matches!(self.place, Some(Place::KeepRow { .. })) {
                    self.place = None;
                }
            }
            // 左侧编辑到一半可能暂时解析不了，保留最后一张图，并显示最新错误。
            Err(error) => self.error = Some(error),
        }
        true
    }

    pub(crate) fn position(&self, (x, y): (f32, f32)) -> (f32, f32) {
        (
            self.body.x + self.body.w / 2.0 + self.pan.0 + x * self.zoom,
            self.body.y + self.body.h / 2.0 + self.pan.1 + y * self.zoom,
        )
    }

    pub(crate) fn place(&mut self) {
        if self.body.w <= 0.0 || self.body.h <= 0.0 {
            return;
        }
        let Some((layout, place)) = self.layout.as_ref().zip(self.place.take()) else {
            return;
        };
        if let Place::KeepRow { card, row, at } = place {
            if let Some(rect) = layout.cards[card] {
                let now = self.position((rect.x, rect.y + (row as f32 + 0.5) * ROW_H));
                self.pan.0 += at.0 - now.0;
                self.pan.1 += at.1 - now.1;
            }
            return;
        }
        let (w, h) = layout.size;
        let fit = ((self.body.w - 32.0) / w)
            .min((self.body.h - 32.0) / h)
            .clamp(MIN_ZOOM, 1.0);
        if matches!(place, Place::Initial) && fit < 0.58 {
            // 大图先显示可读的根卡片，不必先从很小的总览里找放大位置。
            self.zoom = 0.85;
            let root = layout.cards[0].expect("root card");
            self.pan = (
                20.0 - self.body.w / 2.0,
                -(root.y * self.zoom + (root.h * self.zoom / 2.0).min(self.body.h / 2.0 - 16.0)),
            );
        } else {
            self.zoom = fit;
            self.pan = (-w * fit / 2.0, -h * fit / 2.0);
        }
    }

    fn zoom_at(&mut self, factor: f32, (x, y): (f32, f32)) {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let ratio = zoom / self.zoom;
        let (cx, cy) = (
            x - self.body.x - self.body.w / 2.0,
            y - self.body.y - self.body.h / 2.0,
        );
        self.pan = (
            cx - (cx - self.pan.0) * ratio,
            cy - (cy - self.pan.1) * ratio,
        );
        self.zoom = zoom;
    }

    fn step_zoom(&mut self, up: bool) {
        let at = (
            self.body.x + self.body.w / 2.0,
            self.body.y + self.body.h / 2.0,
        );
        self.zoom_at(if up { 1.2 } else { 1.0 / 1.2 }, at);
    }

    fn toggle(&mut self, card: usize, row: usize) {
        if let Some(child) = self
            .graph
            .as_ref()
            .and_then(|graph| graph.get(card)?.get(row)?.child)
            && let Some(rect) = self
                .layout
                .as_ref()
                .and_then(|layout| layout.cards.get(card).copied().flatten())
        {
            if !self.collapsed.remove(&child) {
                self.collapsed.insert(child);
            }
            let at = self.position((rect.x, rect.y + (row as f32 + 0.5) * ROW_H));
            self.place = Some(Place::KeepRow { card, row, at });
            self.layout = None;
        }
    }
}

impl App {
    fn json_key(&self) -> Option<Key> {
        let doc = self.editor.active()?;
        self.json_area().map(|_| (doc.id, doc.revision))
    }

    fn json_area(&self) -> Option<Rect> {
        let doc = self.editor.active()?;
        if doc.preview_kind() != Some(PreviewKind::Json) || doc.is_pending() {
            return None;
        }
        match doc.preview_mode() {
            PreviewMode::Source => None,
            PreviewMode::Split => Some(self.layout.split),
            PreviewMode::Preview => Some(self.layout.editor),
        }
    }

    pub(super) fn advance_json(&mut self) {
        let current = self.json_key();
        let doc = current.map(|(id, _)| id);
        if doc != self.preview.json.doc && matches!(self.pointer.drag, Drag::Json { .. }) {
            self.pointer.drag = Drag::None;
        }
        self.preview.json.select(doc);
        self.poll_json();
        let view = &mut self.preview.json;
        let Some(key) = current.filter(|key| view.checked != Some(*key) && view.pending.is_none())
        else {
            return;
        };
        let Some(rope) = self.editor.active().and_then(|doc| doc.text().rope()) else {
            return;
        };
        if rope.len_bytes() > json::MAX_BYTES {
            view.accept(current, (key, Err(Error::Limit)));
            return;
        }
        let (rope, sender, proxy) = (rope.clone(), view.sender.clone(), self.link.proxy.clone());
        view.pending = Some(key);
        winc_editor::background::spawn(move || {
            let result = json::parse(&rope.to_string());
            if sender.send((key, result)).is_ok()
                && let Some(proxy) = proxy
            {
                let _ = proxy.send_event(UserEvent::ImageReady);
            }
        });
    }

    pub(super) fn poll_json(&mut self) {
        let current = self.json_key();
        if let Ok(done) = self.preview.json.receiver.try_recv() {
            self.preview.json.accept(current, done);
            // 即使丢弃的是过时版本，也要绘制一帧来安排当前版本。
            self.request_redraw();
        }
    }

    pub(in crate::app) fn json_at(&self, x: f32, y: f32) -> Option<Target> {
        let rect = self.json_area()?;
        if rect.w <= 0.0 || rect.h <= 0.0 || !rect.contains(x, y) {
            return None;
        }
        let view = &self.preview.json;
        if self.editor.active().map(|doc| doc.id) != view.doc || rect != view.area {
            return Some(Target::Blank);
        }
        Some(crate::layout::hit(&view.hits, x, y).unwrap_or_else(|| {
            if view.layout.is_some() && view.body.contains(x, y) {
                Target::Canvas
            } else {
                Target::Blank
            }
        }))
    }

    pub(in crate::app) fn click_json(&mut self, hit: Target, x: f32, y: f32) {
        self.focus = Focus::Editor;
        if hit == Target::Canvas && self.click_count(x, y) != 2 {
            self.pointer.drag = Drag::Json { at: (x, y) };
            return;
        }
        let view = &mut self.preview.json;
        match hit {
            Target::Fit | Target::Canvas => view.place = Some(Place::Fit),
            Target::ZoomIn | Target::ZoomOut => view.step_zoom(hit == Target::ZoomIn),
            Target::Toggle { card, row } => view.toggle(card, row),
            Target::Blank => return,
        }
        self.request_redraw();
    }

    pub(in crate::app) fn drag_json(&mut self, (x, y): (f32, f32), (old_x, old_y): (f32, f32)) {
        if let Some((id, _)) = self.json_key() {
            self.pan_json(id, x - old_x, y - old_y);
            self.pointer.drag = Drag::Json { at: (x, y) };
            self.request_redraw();
        }
    }

    pub(in crate::app) fn pan_json(&mut self, id: DocumentId, dx: f32, dy: f32) {
        if self.json_key().is_some_and(|(doc, _)| doc == id) && self.preview.json.doc == Some(id) {
            self.preview.json.pan.0 += dx;
            self.preview.json.pan.1 += dy;
        }
    }

    fn zoom_json_at_pointer(&mut self, power: f32) -> bool {
        let at = self.pointer.mouse;
        if self.overlays.picker.open
            || self.overlays.alert.is_some()
            || self.overlays.form_dialog.is_some()
            || self.popup().is_some()
            || self.settings.open
            || self.json_at(at.0, at.1).is_none()
        {
            return false;
        }
        let view = &mut self.preview.json;
        if view.layout.is_some() && view.body.contains(at.0, at.1) {
            view.zoom_at(power.clamp(-2.0, 2.0).exp(), at);
            self.request_redraw();
        }
        true
    }

    /// 只看结构图时，缩放快捷键作用于画布；分栏时仍调整源代码的字号。
    pub(in crate::app) fn step_json_zoom(&mut self, up: Option<bool>) -> bool {
        if self.json_key().is_none()
            || self
                .editor
                .active()
                .is_some_and(|doc| doc.preview_mode() != PreviewMode::Preview)
        {
            return false;
        }
        let view = &mut self.preview.json;
        if view.layout.is_some() {
            if let Some(up) = up {
                view.step_zoom(up);
            } else {
                view.place = Some(Place::Fit);
            }
            self.request_redraw();
        }
        true
    }

    pub(in crate::app) fn wheel_json(&mut self, delta: MouseScrollDelta) -> bool {
        if !self.command_held() {
            return false;
        }
        let power = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * 0.18,
            MouseScrollDelta::PixelDelta(delta) => {
                let scale = self
                    .frame
                    .renderer
                    .as_ref()
                    .map_or(1.0, winc_render::Renderer::scale_factor);
                delta.y as f32 / scale * 0.004
            }
        };
        self.zoom_json_at_pointer(power)
    }

    pub(in crate::app) fn json_pinch(&mut self, delta: f32) -> bool {
        let zoom = |app: &mut Self| app.zoom_json_at_pointer(delta);
        let (x, y) = self.pointer.mouse;
        if let Some(pane) = self.background_pane_at(x, y) {
            self.with_pane(pane, zoom)
        } else {
            zoom(self)
        }
    }

    #[cfg(test)]
    pub(in crate::app) fn wait_for_json(&mut self) {
        while self.preview.json.pending.is_some() {
            let done = self
                .preview
                .json
                .receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let current = self.json_key();
            self.preview.json.accept(current, done);
            self.advance_json();
        }
    }
}
