//! Markdown 预览的滚动：滚轮、目录与锚点的跳转、表格的横向滚动，以及与源代码一侧的同步。

use std::time::{Duration, Instant};

use winc_render::Rect;

use super::*;
use crate::app::animation::Clock;

/// 标题列表中列出的范围：标题超过可显示的行数时只列出连续的一段，可按行滚动。
#[derive(Debug, Clone, Copy, Default)]
struct TocList {
    /// 上一帧卡片的矩形；列表收起时为空。
    card: Rect,
    /// 上一帧各行所占的矩形，即滚动条的轨道。
    track: Rect,
    /// 列出的第一行、行数和标题的总数。
    first: usize,
    rows: usize,
    total: usize,
    /// 一行的高度（逻辑像素）。
    row: f32,
    /// 列出的第一行相对默认位置的偏移（行）；默认位置使当前所在的一节居中。
    shift: isize,
    /// 设置 `shift` 时当前所在的一节；该值变化时 `shift` 归零。
    at: Option<usize>,
    /// 滚轮位移中不足一行的部分（逻辑像素）。
    wheel: f32,
}

impl TocList {
    /// 返回本帧列出的第一行：默认位置 `natural` 加上偏移，并使 `rows` 行不超出 `total`。
    fn start(
        &mut self,
        natural: usize,
        current: Option<usize>,
        (rows, total): (usize, usize),
    ) -> usize {
        if self.at != current {
            (self.at, self.shift, self.wheel) = (current, 0, 0.0);
        }
        let start = natural.saturating_add_signed(self.shift).min(total - rows);
        self.shift = start as isize - natural as isize;
        (self.first, self.rows, self.total) = (start, rows, total);
        start
    }

    /// 返回滚动条的（滚动位置，内容高度），逻辑像素；全部标题都已列出时为 `None`。
    fn scrollbar(&self) -> Option<(f32, f32)> {
        (self.rows < self.total)
            .then_some((self.first as f32 * self.row, self.total as f32 * self.row))
    }

    /// 记录本帧展开卡片的矩形、行高与滚动条轨道，返回滚动条参数。
    fn place_geometry(
        &mut self,
        card: Rect,
        pad: f32,
        row_h: f32,
        rows: usize,
    ) -> (Rect, Option<(f32, f32)>) {
        let track = Rect::new(card.x, card.y + pad, card.w, rows as f32 * row_h);
        self.card = card;
        self.row = row_h;
        self.track = track;
        (track, self.scrollbar())
    }

    /// 拖动标题列表滚动条，返回是否更新了滚动偏移。
    fn drag_scrollbar(&mut self, y: f32, grab: f32) -> bool {
        let Some((_, content)) = self.scrollbar() else {
            return false;
        };
        let track = self.track;
        let Some(thumb) = crate::layout::scrollbar_thumb(track, 0.0, content) else {
            return false;
        };
        let max = content - track.h;
        let to = crate::layout::scroll_for_thumb(track.y, track.h, thumb.h, y, grab, max);
        let first = (to / self.row).round() as isize;
        self.shift += first - self.first as isize;
        true
    }

    /// 按滚轮位移 `dy` 滚动标题列表，返回是否已处理。
    fn scroll_wheel(&mut self, (x, y): (f32, f32), dy: f32) -> bool {
        if self.rows >= self.total || self.row <= 0.0 || !self.card.contains(x, y) {
            return false;
        }
        self.wheel -= dy;
        let steps = (self.wheel / self.row).trunc();
        self.wheel -= steps * self.row;
        self.shift += steps as isize;
        true
    }

    /// 检测窗口坐标 `(x, y)` 是否落在滚动条命中区域内，命中时返回抓取偏移。
    fn scrollbar_hit(&self, x: f32, y: f32, hit_width: f32) -> Option<f32> {
        if !self.track.contains(x, y) || x < self.track.right() - hit_width {
            return None;
        }
        let (scroll, content) = self.scrollbar()?;
        let thumb = crate::layout::scrollbar_thumb(self.track, scroll, content)?;
        Some(if (thumb.y..=thumb.bottom()).contains(&y) {
            y - thumb.y
        } else {
            thumb.h / 2.0
        })
    }

    #[cfg(test)]
    fn track_rect(&self) -> Rect {
        self.track
    }

    #[cfg(test)]
    fn shift_rows(&self) -> isize {
        self.shift
    }
}

/// 本帧标题列表的显示与过渡状态，供视图绘制使用。
#[derive(Debug, Clone, Copy)]
pub(crate) struct TocDisplay {
    /// 列表当前是否钉住常驻。
    pub(crate) stays: bool,
    /// 列表是否处于展开或收起过渡中。
    pub(crate) open: bool,
    /// 列表是否响应鼠标点击与滚动。
    pub(crate) live: bool,
    /// 钉住的程度（`0.0` 为悬停卡片，`1.0` 为钉住常驻）。
    pub(crate) pinning: f32,
    /// 展开或收起过渡的进度（`0.0` 到 `1.0`）。
    pub(crate) progress: f32,
}

/// 计算展开态标题列表几何所需的本帧参数。
#[derive(Debug, Clone, Copy)]
pub(crate) struct TocListLayout {
    /// 预览区域（窗口坐标，逻辑像素）。
    pub(crate) area: Rect,
    /// 卡片左边横坐标。
    pub(crate) x: f32,
    /// 卡片宽度。
    pub(crate) w: f32,
    /// 本帧显示的行数与标题总数。
    pub(crate) rows: usize,
    pub(crate) total: usize,
    /// 默认起始行号和当前所在标题序号。
    pub(crate) natural: usize,
    pub(crate) current: Option<usize>,
    /// 钉住时的顶边位置。
    pub(crate) docked_y: f32,
    /// 卡片内边距与单行高度。
    pub(crate) pad: f32,
    pub(crate) row_h: f32,
    /// 钉住程度与是否常驻。
    pub(crate) pinning: f32,
    pub(crate) stays: bool,
    /// 指示条中可见标题数（用于锚点定位）。
    pub(crate) shown: usize,
}

/// 展开态标题列表的本帧布局结果。
#[derive(Debug, Clone, Copy)]
pub(crate) struct TocListFrame {
    /// 本帧列出的第一行标题序号。
    pub(crate) start: usize,
    /// 卡片矩形。
    pub(crate) card: Rect,
    /// 列表内容轨道矩形（也是滚动条轨道）。
    pub(crate) track: Rect,
    /// 滚动条状态（滚动位置，内容高度）；全部列出时为 `None`。
    pub(crate) scrollbar: Option<(f32, f32)>,
}

/// Markdown 预览左侧的标题指示条与可展开目录状态机。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TocPanel {
    /// 标题列表展开时淡入的时钟；不在淡入中时为 `None`。
    fade: Option<Clock>,
    /// 鼠标进入标题指示条的时刻，满 `TOC_HOVER_DELAY` 后列表展开；不在区域内时为 `None`。
    rest: Option<Instant>,
    /// 本次停留是否已展开标题列表。
    shown: bool,
    /// 正文左边缘在钉住过渡结束后的位置（窗口坐标，逻辑像素）。
    left: f32,
    /// 钉住过渡开始时正文左边缘的位置；不在过渡中时为 `None`。
    slide: Option<f32>,
    /// 标题列表收起过渡的时钟；不在收起中时为 `None`。
    close: Option<Clock>,
    /// 悬停展开的标题列表上边缘相对预览区域上边缘的位置；收起后为 `None`。
    top: Option<f32>,
    /// 标题列表钉住（`true`）或取消钉住（`false`）的过渡时钟；不在过渡中时为 `None`。
    pin: Option<(bool, Clock)>,
    /// 在标题列表中点击过的标题序号和点击后的滚动位置。
    picked: Option<(usize, f32)>,
    /// 上一帧标题指示条标出的当前所在一节序号；没有时为 `None`。
    current: Option<usize>,
    /// 点击过的标题在正文中的高亮底色：标题序号与淡出时钟。
    flash: Option<(usize, Clock)>,
    /// 展开的标题列表滚动窗口。
    list: TocList,
}

impl TocPanel {
    /// 清除已经播放完成的目录相关过渡时钟。
    pub(super) fn settle(&mut self) {
        if self.fade.is_some_and(|clock| clock.done()) {
            self.fade = None;
        }
        if self.close.is_some_and(|clock| clock.done()) {
            self.close = None;
        }
        if self.pin.is_some_and(|(_, clock)| clock.done()) {
            self.pin = None;
        }
        if self.flash.is_some_and(|(_, clock)| clock.done()) {
            self.flash = None;
        }
    }

    /// 是否有目录过渡动画正在进行。
    pub(super) fn animating(&self) -> bool {
        self.fade.is_some() || self.close.is_some() || self.pin.is_some() || self.flash.is_some()
    }

    /// 鼠标进出标题指示条/列表区域时更新悬停计时与收起过渡。
    pub(crate) fn on_hover_change(
        &mut self,
        on_toc: bool,
        close_clock: impl FnOnce() -> Option<Clock>,
    ) {
        if self.shown {
            self.close = close_clock();
        }
        self.rest = on_toc.then(Instant::now);
        self.shown = false;
    }

    /// 在定时唤醒时检查悬停是否满 `TOC_HOVER_DELAY`，返回 `(是否刚展开, 下次唤醒时刻)`。
    pub(crate) fn poll_reveal(
        &mut self,
        now: Instant,
        fade_clock: impl FnOnce() -> Option<Clock>,
    ) -> (bool, Option<Instant>) {
        let Some(since) = self.rest.filter(|_| !self.shown) else {
            return (false, None);
        };
        let at = since + crate::theme::TOC_HOVER_DELAY;
        if now >= at {
            self.shown = true;
            self.close = None;
            self.fade = fade_clock();
            (true, None)
        } else {
            (false, Some(at))
        }
    }

    /// 切换钉住状态，启动钉住与正文左边缘滑移过渡。
    fn toggle_pin(&mut self, pinned: bool, clock: Option<Clock>, body_left: Option<f32>) {
        self.pin = clock.map(|clock| (pinned, clock));
        self.slide = body_left.filter(|_| clock.is_some());
    }

    /// 记录在标题列表中点击的标题，并启动正文目标标题的淡出高亮。
    fn pick_heading(&mut self, index: usize, scroll: f32, clock: Option<Clock>) {
        self.picked = Some((index, scroll));
        self.flash = clock.map(|clock| (index, clock));
    }

    /// 记录正文目标左边缘并返回钉住过渡中的当前左边缘。
    pub(crate) fn body_left(&mut self, target_left: f32) -> f32 {
        self.left = target_left;
        match (self.pin, self.slide) {
            (Some((_, clock)), Some(from)) => clock.between(from, target_left),
            _ => {
                self.slide = None;
                target_left
            }
        }
    }

    /// 没有可显示的标题指示条时重置本帧当前节记录。
    pub(crate) fn reset_inactive(&mut self) {
        self.current = None;
        self.top = None;
        self.list = TocList::default();
    }

    /// 根据当前滚动位置和各标题纵坐标确定当前所在的一节，并写回面板状态。
    pub(crate) fn resolve_current(
        &mut self,
        scroll: f32,
        max: f32,
        visible_bottom: f32,
        tops: &[f32],
    ) -> Option<usize> {
        let picked = self
            .picked
            .filter(|&(i, at)| i < tops.len() && (at - scroll).abs() < 0.5)
            .map(|(i, _)| i);
        if picked.is_none() {
            self.picked = None;
        }
        let current = picked.or_else(|| {
            if max > 0.0 && scroll >= max - 0.5 {
                tops.iter().rposition(|&top| top < visible_bottom)
            } else {
                tops.iter().rposition(|&top| top <= scroll + 1.0)
            }
        });
        self.current = current;
        current
    }

    /// 返回当前正在淡出的跳转标题高亮 `(标题序号, 透明度系数)`。
    pub(crate) fn flash_highlight(&self) -> Option<(usize, f32)> {
        let (index, clock) = self.flash?;
        Some((index, 0.14 * (1.0 - clock.linear())))
    }

    /// 计算本帧标题列表的悬停、钉住与展开进度。
    pub(crate) fn display_state(
        &self,
        pinned: bool,
        on_toc_or_bar: bool,
        area_x: f32,
        gutter: f32,
        min_width: f32,
    ) -> TocDisplay {
        let rested = self
            .rest
            .is_some_and(|since| since.elapsed() >= crate::theme::TOC_HOVER_DELAY);
        let hovered = rested && on_toc_or_bar;
        let closing = self.close.map(|clock| 1.0 - clock.progress());
        let fits_at = |left: f32| left - gutter - 8.0 - (area_x + 4.0) >= min_width;
        let fits = fits_at(self.left);
        let fitted = self.slide.is_some_and(fits_at);
        let stays = pinned && fits;
        let pinning = match self.pin {
            Some((to, clock)) if fits || (!to && fitted) => {
                let t = clock.progress();
                if to { t } else { 1.0 - t }
            }
            _ => f32::from(u8::from(stays)),
        };
        let open = hovered || stays || closing.is_some();
        let live = hovered || stays;
        let progress = match (stays, hovered, closing) {
            (true, ..) => 1.0,
            (false, false, Some(left)) => left,
            _ => self.fade.map_or(1.0, |c| c.progress()),
        };
        TocDisplay {
            stays,
            open,
            live,
            pinning,
            progress,
        }
    }

    /// 收起标题列表时重置悬停锚点与滚动窗口。
    pub(crate) fn close_list(&mut self) {
        self.top = None;
        self.list = TocList::default();
    }

    /// 计算并记录本帧展开态标题列表的起始行、卡片矩形、轨道与滚动条状态。
    pub(crate) fn layout_list(
        &mut self,
        input: TocListLayout,
        bar_y: impl Fn(usize) -> f32,
    ) -> TocListFrame {
        let start = self
            .list
            .start(input.natural, input.current, (input.rows, input.total));
        let h = input.rows as f32 * input.row_h + input.pad * 2.0;
        let anchor = input
            .current
            .filter(|&c| c < input.shown && (start..start + input.rows).contains(&c))
            .unwrap_or(start);
        let bar_mid = bar_y(anchor.min(input.shown.saturating_sub(1)));
        let row_mid = input.pad + (anchor - start) as f32 * input.row_h + input.row_h / 2.0;
        let fit_in = |y: f32| y.min(input.area.bottom() - 8.0 - h).max(input.area.y + 8.0);
        let floating = self.top.map_or(bar_mid - row_mid, |top| input.area.y + top);
        if !input.stays {
            self.top = Some(fit_in(floating) - input.area.y);
        }
        let y = fit_in(floating + (input.docked_y - floating) * input.pinning);
        let card = Rect::new(input.x, y, input.w.max(0.0), h);
        let (track, scrollbar) = self
            .list
            .place_geometry(card, input.pad, input.row_h, input.rows);
        TocListFrame {
            start,
            card,
            track,
            scrollbar,
        }
    }

    /// 拖动标题列表滚动条，返回是否更新了滚动偏移。
    fn drag_scrollbar(&mut self, y: f32, grab: f32) -> bool {
        self.list.drag_scrollbar(y, grab)
    }

    /// 按滚轮位移 `dy` 滚动标题列表，返回是否已处理。
    fn scroll_wheel(&mut self, at: (f32, f32), dy: f32) -> bool {
        self.list.scroll_wheel(at, dy)
    }

    /// 检测窗口坐标 `(x, y)` 是否落在标题列表的滚动条命中区域内，命中时返回抓取偏移。
    pub(crate) fn scrollbar_hit(&self, x: f32, y: f32, hit_width: f32) -> Option<f32> {
        self.list.scrollbar_hit(x, y, hit_width)
    }

    #[cfg(test)]
    pub(crate) fn current_heading(&self) -> Option<usize> {
        self.current
    }

    #[cfg(test)]
    pub(crate) fn floating_top(&self) -> Option<f32> {
        self.top
    }

    #[cfg(test)]
    pub(crate) fn track_rect(&self) -> Rect {
        self.list.track_rect()
    }

    #[cfg(test)]
    pub(crate) fn shift_rows(&self) -> isize {
        self.list.shift_rows()
    }

    #[cfg(test)]
    pub(crate) fn set_rest_for_test(&mut self, at: Instant) {
        self.rest = Some(at);
    }

    #[cfg(test)]
    pub(crate) fn set_fade_for_test(&mut self, clock: Option<Clock>) {
        self.fade = clock;
    }

    #[cfg(test)]
    pub(crate) fn set_flash_for_test(&mut self, index: usize, clock: Clock) {
        self.flash = Some((index, clock));
    }
}

/// 分帧排版的状态，见 [`Later`](super::laid::Later)。
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CatchUp {
    /// 一帧中排版视口之外的块的时限；`None` 时取
    /// [`LAYOUT_BUDGET`](super::laid::LAYOUT_BUDGET)，测试中不设时限。
    budget: Option<Duration>,
    /// 本帧绘制后是否仍有沿用旧结果的块；为真时逐帧重绘，直到全部排完。
    pending: bool,
}

impl CatchUp {
    /// 返回本帧排版视口之外的块的截止时刻。
    fn deadline(&self) -> Instant {
        let default = if cfg!(test) {
            Duration::from_secs(3600)
        } else {
            super::laid::LAYOUT_BUDGET
        };
        Instant::now() + self.budget.unwrap_or(default)
    }
}

impl MarkdownPreview {
    /// 开始新的一帧绘制前，清除上一帧遗留的分帧补排标记。
    pub(crate) fn begin_frame(&mut self) {
        self.catch_up.pending = false;
    }

    /// 返回本帧分帧排版视口外块的截止时刻。
    pub(crate) fn catch_up_deadline(&self) -> Instant {
        self.catch_up.deadline()
    }

    /// 本帧排版完成后记录是否仍有未补排的块。
    pub(crate) fn finish_catch_up(&mut self, pending: bool) {
        self.catch_up.pending = pending;
    }

    /// 预览是否有正在进行的过渡动画或分帧补排任务。
    pub(crate) fn animating(&self) -> bool {
        self.toc.animating() || self.catch_up.pending || self.anchor_flash.is_some()
    }

    /// 清除已经播放完成的预览过渡时钟。
    pub(crate) fn settle_animations(&mut self) {
        self.toc.settle();
        if self.anchor_flash.is_some_and(|(_, clock)| clock.done()) {
            self.anchor_flash = None;
        }
        if self.motion.as_ref().is_some_and(|m| m.clock.done()) {
            self.motion = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn set_catch_up_budget(&mut self, budget: Option<Duration>) {
        self.catch_up.budget = budget;
    }

    #[cfg(test)]
    pub(crate) fn is_catch_up_pending(&self) -> bool {
        self.catch_up.pending
    }

    /// 窗口横坐标 `x` 是否落在预览的内容一栏内。
    pub(crate) fn in_content(&self, x: f32) -> bool {
        self.layout.as_ref().is_none_or(|laid| laid.in_content(x))
    }

    /// 预览上窗口坐标 `(x, y)` 处是整份文字里的第几个字符。
    pub(crate) fn offset_at(&self, x: f32, y: f32) -> Option<usize> {
        self.layout
            .as_ref()?
            .offset_at(self.area.y, self.scroll(), x, y)
    }

    /// 预览上窗口坐标 `(x, y)` 处的字在原文中的字节偏移。
    pub(crate) fn source_at(&self, x: f32, y: f32) -> Option<usize> {
        self.layout
            .as_ref()?
            .source_at(self.area.y, self.scroll(), x, y)
    }

    /// 把窗口坐标 `(x, y)` 处可横向滚动的表格横向滚动 `dx`，返回是否滚动了。
    pub(crate) fn scroll_table(&mut self, x: f32, y: f32, dx: f32) -> bool {
        let (area_y, scroll) = (self.area.y, self.scroll());
        let Some(key) = self
            .layout
            .as_mut()
            .and_then(|laid| laid.scroll_table(area_y, scroll, x, y, dx))
        else {
            return false;
        };
        self.last_table = Some(key);
        true
    }

    /// 整份文字里第 `offset` 个字符所在那一行的上边缘。
    pub(crate) fn offset_top(&self, offset: usize) -> Option<f32> {
        self.layout.as_ref()?.offset_top(offset)
    }

    /// 整份排版文字（各块文字拼起来）。
    pub(crate) fn full_text(&self) -> Option<String> {
        self.layout.as_ref().map(laid::Laid::full_text)
    }

    /// 锚点 `anchor`（标题、脚注、脚注引用）的上边缘和高度。
    pub(crate) fn anchor_box(&self, anchor: &str) -> Option<(f32, f32)> {
        let anchor = percent_decode(anchor).to_lowercase();
        self.layout.as_ref()?.anchor_box(&anchor)
    }

    /// 锚点 `anchor` 在内容里的高度。
    pub(crate) fn anchor_top(&self, anchor: &str) -> Option<f32> {
        let anchor = percent_decode(anchor).to_lowercase();
        self.layout.as_ref()?.anchor_top(&anchor)
    }
}

impl App {
    /// 拖动标题列表的滚动条：鼠标的纵坐标为 `y`，按下处距滑块上边 `grab`（逻辑像素）。
    pub(crate) fn drag_toc_scrollbar(&mut self, y: f32, grab: f32) {
        if self.preview.markdown.toc.drag_scrollbar(y, grab) {
            self.request_redraw();
        }
    }

    /// 按滚轮位移 `dy` 滚动标题列表，返回是否已处理。
    ///
    /// 鼠标 (`x`, `y`) 不在列表上，或全部标题都已列出时不处理。
    pub(crate) fn scroll_toc(&mut self, at: (f32, f32), dy: f32) -> bool {
        if self.preview.markdown.toc.scroll_wheel(at, dy) {
            self.request_redraw();
            return true;
        }
        false
    }

    /// 排版完成后，若有属于该文档的待跳转标题（见 [`App::open_markdown_file`]），滚动到该标题。
    pub(crate) fn scroll_to_pending_anchor(&mut self) {
        let md = &mut self.preview.markdown;
        let Some((_, anchor)) = md.pending_anchor.take_if(|(id, _)| md.doc == Some(*id)) else {
            return;
        };
        match self.preview.markdown.anchor_top(&anchor) {
            Some(y) => self.scroll_markdown_to(y),
            None => self.flash(t!("preview-markdown-heading-not-found")),
        }
    }

    /// 可横向滚动的表格 `key`（见 [`TableBar::key`]）当前的横向滚动位置（逻辑像素）。
    pub(crate) fn markdown_table_scroll(&self, key: (usize, usize)) -> f32 {
        (self.preview.markdown.layout.as_ref()).map_or(0.0, |laid| laid.hscroll_of(key.0, key.1))
    }

    /// 拖动表格的水平滚动条：按鼠标横坐标 `x` 和按下时在滑块上的位置 `grab` 设置滚动位置。
    pub(in crate::app) fn drag_markdown_table_scrollbar(&mut self, x: f32, grab: f32) {
        let Some(bar) = self.hits.markdown_table_bar else {
            return;
        };
        let scroll = self.markdown_table_scroll(bar.key);
        let Some(thumb) = crate::layout::hscrollbar_thumb(bar.track, scroll, bar.content) else {
            return;
        };
        let max = bar.content - bar.track.w;
        let to = crate::layout::scroll_for_thumb(bar.track.x, bar.track.w, thumb.w, x, grab, max);
        if let Some(laid) = self.preview.markdown.layout.as_mut() {
            laid.set_hscroll(bar.key, to.clamp(0.0, max));
        }
        self.preview.markdown.last_table = Some(bar.key);
        self.request_redraw();
    }

    pub(crate) fn scroll_markdown(&mut self, dy: f32) {
        let y = self.preview.markdown.scroll() - dy;
        self.scroll_markdown_to(y);
    }

    /// 处理标题指示条中第 `i` 个标题的点击：预览滚动到该标题。
    ///
    /// 滚动后将其记为当前所在的一节，并在正文中为其添加逐渐淡出的底色，标示跳转的目标。
    pub(in crate::app) fn press_toc(&mut self, target: TocTarget) {
        if target == TocTarget::Pin {
            self.update_state(|state| state.markdown_toc_pinned = !state.markdown_toc_pinned);
            let pinned = self.state.markdown_toc_pinned;
            let clock = self.clock(crate::app::animation::TOC_PIN);
            // 钉住状态变化使正文左边缘移动时，从当前位置过渡到新位置。
            let left = self
                .preview
                .markdown
                .layout
                .as_ref()
                .map(|laid| laid.column_bounds().0);
            self.preview.markdown.toc.toggle_pin(pinned, clock, left);
            self.request_redraw();
            return;
        }
        let TocTarget::Heading(i) = target else {
            return;
        };
        let Some(&top) = self.hits.markdown_toc_tops.get(i) else {
            return;
        };
        self.glide_pane_to(ScrollTarget::Markdown, top);
        let scroll = self.preview.markdown.scroll();
        let clock = self.clock(crate::app::animation::TOC_FLASH);
        self.preview.markdown.toc.pick_heading(i, scroll, clock);
    }

    /// 跳转到预览中的锚点 `anchor`（标题、脚注、脚注引用）；找不到时提示。
    ///
    /// 标题滚动到预览顶部。脚注和脚注引用滚动到预览上部三分之一处，上方保留一段上下文，并像
    /// 点击标题列表一样铺一层逐渐淡出的底色，标示跳转的目标：脚注集中在文末，滚动到底之后
    /// 仅凭位置看不出是哪一条。
    pub(in crate::app) fn jump_to_anchor(&mut self, anchor: &str) {
        let Some((top, height)) = self.preview.markdown.anchor_box(anchor) else {
            self.flash(t!("preview-markdown-heading-not-found"));
            return;
        };
        let lower = anchor.to_lowercase();
        if !(lower.starts_with("fn-") || lower.starts_with("fnref-")) {
            // 书中不用过渡：过渡中的滚动会被当成用户滚动，不再对齐。
            if self.hold_book_at(top) {
                self.settle_glide_of(ScrollTarget::Markdown);
                self.scroll_markdown_to(top);
                return;
            }
            self.glide_pane_to(ScrollTarget::Markdown, top);
            return;
        }
        self.glide_pane_to(
            ScrollTarget::Markdown,
            top - self.preview.markdown.area.h / 3.0,
        );
        let clock = self.clock(crate::app::animation::TOC_FLASH);
        self.preview.markdown.anchor_flash = clock.map(|clock| ((top, height), clock));
    }

    /// 将预览滚动到 `y`；分栏时左侧的文字随之滚动到对应位置。
    pub(crate) fn scroll_markdown_to(&mut self, y: f32) {
        self.preview.markdown.scroll_to(y);
        self.request_redraw();
        if self.markdown_shown_area() != Some(self.layout.split) {
            return;
        }
        let max = self.max_scroll_line();
        let y = self.preview.markdown.scroll();
        let (Some(laid), Some(doc)) = (
            self.preview.markdown.layout.as_ref(),
            self.editor.active_mut(),
        ) else {
            return;
        };
        let line = laid.line_of_y(y, doc.text().line_count());
        set_scroll_line(doc, text_row(doc, line).clamp(0.0, max.max(0.0)));
        self.preview.markdown.synced = Some((doc.id, doc.scroll_line, laid.height()));
    }

    /// 滚动文字，使原文第 `line` 行（小数）位于顶部。
    pub(in crate::app) fn scroll_text_to(&mut self, line: f32) {
        let max = self.max_scroll_line();
        if let Some(doc) = self.editor.active_mut() {
            set_scroll_line(doc, text_row(doc, line).clamp(0.0, max.max(0.0)));
        }
    }

    /// 将预览滚动到文字顶端那一行对应的位置；仅在刚切换为只显示预览后执行一次。
    ///
    /// 位置的计算使用分栏同步滚动的映射。
    ///
    /// 排版完成之后、绘制之前调用。
    pub(crate) fn align_markdown_once(&mut self) {
        let (Some(laid), Some(doc)) = (self.preview.markdown.layout.as_ref(), self.editor.active())
        else {
            return;
        };
        if self.preview.markdown.align != Some(doc.id) {
            return;
        }
        self.preview.markdown.align = None;
        let y = laid.y_of_line(text_line(doc), doc.text().line_count());
        self.preview.markdown.scroll_to(y);
    }

    /// 分栏时左侧文字滚动或预览重排后，将右侧滚动到对应位置。
    ///
    /// 绘制预览之前调用。
    pub(crate) fn sync_markdown_scroll(&mut self) {
        let (Some(laid), Some(doc)) = (self.preview.markdown.layout.as_ref(), self.editor.active())
        else {
            return;
        };
        let now = (doc.id, doc.scroll_line, laid.height());
        if self.preview.markdown.synced == Some(now) {
            return;
        }
        let y = laid.y_of_line(text_line(doc), doc.text().line_count());
        self.preview.markdown.scroll_to(y);
        self.preview.markdown.synced = Some(now);
    }
}
