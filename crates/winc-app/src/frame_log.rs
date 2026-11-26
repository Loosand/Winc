//! 帧日志与单帧分阶段计时：设置环境变量 `WINC_FRAME_LOG` 后，每隔 [`REPORT_EVERY`] 向标准错误
//! 输出一行绘制统计与解析/排版/绘制/呈现的分阶段耗时及慢帧主导阶段分布。
//!
//! 用于排查卡顿和掉帧：在终端中启动 winc，复现问题后附上输出。只统计实际绘制的帧；画面静止、
//! 不绘制时不输出。未启用测量与帧日志时，分阶段计时路径不触发系统时钟调用。
//!
//! 另记录鼠标移动到画面更新的间隔、查询光标下的窗口的耗时、跳过的帧数和被系统收回的呈现缓冲数。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// 两次输出之间的时长。
///
/// 取自调参：足以收集上百帧作统计，卡顿发生后 5 s 内即有输出。
const REPORT_EVERY: Duration = Duration::from_secs(5);
/// 慢帧环形缓冲保留的最近超时帧上限（固定有界，避免长时间运行累积内存）。
const SLOW_FRAME_CAP: usize = 32;

/// 渲染帧的四个主要阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePhase {
    /// 文档解析（如 Markdown 同步或局部增量解析）。
    Parse = 0,
    /// 界面与文档排版（`relayout` 及 Markdown 块级排版）。
    Layout = 1,
    /// CPU 画布光栅化与界面绘制。
    Draw = 2,
    /// 呈现缓冲获取、解锁与提交到系统合成器。
    Present = 3,
}

/// 单帧的分阶段耗时分解。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FramePhases {
    /// 解析阶段耗时。
    pub parse: Duration,
    /// 排版阶段耗时。
    pub layout: Duration,
    /// 绘制阶段耗时（已扣除绘制回调内部的解析与排版耗时）。
    pub draw: Duration,
    /// 呈现阶段耗时（缓冲获取与合成器提交）。
    pub present: Duration,
    /// 本帧总耗时（四阶段之和，测量按键出帧时另计入按键处理耗时）。
    pub total: Duration,
}

impl FramePhases {
    /// 返回本帧四个阶段中耗时最长的主导阶段，用于慢帧归因。
    pub fn dominant(self) -> FramePhase {
        let mut best = (FramePhase::Parse, self.parse);
        for candidate in [
            (FramePhase::Layout, self.layout),
            (FramePhase::Draw, self.draw),
            (FramePhase::Present, self.present),
        ] {
            if candidate.1 > best.1 {
                best = candidate;
            }
        }
        best.0
    }

    /// 构造仅包含总耗时、未细分阶段的记录（供单测或无细分来源使用）。
    #[cfg(test)]
    pub fn total_only(total: Duration) -> Self {
        Self {
            draw: total,
            total,
            ..Self::default()
        }
    }
}

/// 单帧内收集解析、排版、绘制与呈现阶段耗时的计量器。
#[derive(Default)]
pub struct FramePhaseMeter {
    /// 本帧是否处于计时开启状态。
    active: bool,
    /// 累计的解析阶段耗时。
    parse: Duration,
    /// 累计的排版阶段耗时。
    layout: Duration,
    /// 发生在 `Renderer::draw` 回调内部的解析与排版耗时，用于避免与绘制阶段重复计数。
    in_draw_sub: Duration,
}

impl FramePhaseMeter {
    /// 开始新一帧的阶段计时；`active` 为假时清零并跳过全部计时。
    pub fn begin(&mut self, active: bool) {
        self.active = active;
        self.parse = Duration::ZERO;
        self.layout = Duration::ZERO;
        self.in_draw_sub = Duration::ZERO;
    }

    /// 记录一段解析耗时；`in_draw` 表示该解析发生在 `Renderer::draw` 回调内部。
    pub fn add_parse(&mut self, elapsed: Duration, in_draw: bool) {
        if self.active {
            self.parse += elapsed;
            if in_draw {
                self.in_draw_sub += elapsed;
            }
        }
    }

    /// 记录一段排版耗时；`in_draw` 表示该排版发生在 `Renderer::draw` 回调内部。
    pub fn add_layout(&mut self, elapsed: Duration, in_draw: bool) {
        if self.active {
            self.layout += elapsed;
            if in_draw {
                self.in_draw_sub += elapsed;
            }
        }
    }

    /// 结束当前帧并汇总四阶段耗时。
    ///
    /// `draw_wall` 为进入 `Renderer::draw` 到返回的总耗时（含回调内的解析、排版与呈现）；
    /// `present` 为呈现器单独测得的呈现缓冲获取与提交耗时。
    pub fn finish(&mut self, draw_wall: Duration, present: Duration) -> FramePhases {
        let active = self.active;
        self.active = false;
        if !active {
            return FramePhases {
                draw: draw_wall,
                total: draw_wall,
                ..FramePhases::default()
            };
        }
        let draw = draw_wall.saturating_sub(self.in_draw_sub + present);
        let total = self.parse + self.layout + draw + present;
        FramePhases {
            parse: self.parse,
            layout: self.layout,
            draw,
            present,
            total,
        }
    }
}

thread_local! {
    /// 当前线程是否开启单帧分阶段计时（关闭时快路径仅读取此标志，不借用 `RefCell`）。
    static PHASE_ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// 当前线程是否位于 `Renderer::draw` 回调内部。
    static PHASE_IN_DRAW: Cell<bool> = const { Cell::new(false) };
    /// 当前线程正在绘制的帧的阶段计量器。
    static PHASE_METER: RefCell<FramePhaseMeter> = RefCell::new(FramePhaseMeter::default());
}

/// 开始记录当前线程本帧的分阶段耗时；`active` 为假时保持零开销关闭状态。
pub fn begin_frame(active: bool) {
    PHASE_ACTIVE.set(active);
    PHASE_IN_DRAW.set(false);
    if active {
        PHASE_METER.with(|m| m.borrow_mut().begin(true));
    }
}

/// 取消当前帧的阶段计时（例如呈现缓冲被占满而跳过本帧时）。
pub fn cancel_frame() {
    PHASE_IN_DRAW.set(false);
    if PHASE_ACTIVE.replace(false) {
        PHASE_METER.with(|m| m.borrow_mut().begin(false));
    }
}

/// 在 `Renderer::draw` 回调作用域内执行 `f`，使回调内产生的解析与排版耗时不与绘制阶段重复计数。
pub fn in_draw_phase<T>(f: impl FnOnce() -> T) -> T {
    if !PHASE_ACTIVE.get() {
        return f();
    }
    let prev = PHASE_IN_DRAW.replace(true);
    let result = f();
    PHASE_IN_DRAW.set(prev);
    result
}

/// 执行解析闭包 `f`，在分阶段计时开启时将其耗时计入本帧的解析阶段。
pub fn time_parse<T>(f: impl FnOnce() -> T) -> T {
    if !PHASE_ACTIVE.get() {
        return f();
    }
    let started = Instant::now();
    let result = f();
    let elapsed = started.elapsed();
    let in_draw = PHASE_IN_DRAW.get();
    PHASE_METER.with(|m| m.borrow_mut().add_parse(elapsed, in_draw));
    result
}

/// 执行排版闭包 `f`，在分阶段计时开启时将其耗时计入本帧的排版阶段。
pub fn time_layout<T>(f: impl FnOnce() -> T) -> T {
    if !PHASE_ACTIVE.get() {
        return f();
    }
    let started = Instant::now();
    let result = f();
    let elapsed = started.elapsed();
    let in_draw = PHASE_IN_DRAW.get();
    PHASE_METER.with(|m| m.borrow_mut().add_layout(elapsed, in_draw));
    result
}

/// 结束当前帧的阶段计时并返回四阶段耗时分解。
pub fn finish_frame(draw_wall: Duration, present: Duration) -> FramePhases {
    PHASE_IN_DRAW.set(false);
    if !PHASE_ACTIVE.replace(false) {
        return FramePhases {
            draw: draw_wall,
            total: draw_wall,
            ..FramePhases::default()
        };
    }
    PHASE_METER.with(|m| m.borrow_mut().finish(draw_wall, present))
}

/// 单次超时慢帧的结构化诊断记录（不包含任何文档正文或文件路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlowFrameRecord {
    /// 自本次会话开始以来的帧序号（从 1 起）。
    pub frame_index: u64,
    /// 本帧四阶段及总耗时分解。
    pub phases: FramePhases,
    /// 本帧耗时最长的主导阶段。
    pub dominant: FramePhase,
    /// 触发慢帧判定时的屏幕刷新间隔。
    pub refresh: Duration,
    /// 该帧的物理像素尺寸。
    pub size: (u32, u32),
}

/// 帧日志的状态；未启用时不记录。
pub struct FrameLog {
    /// 是否启用（设置了 `WINC_FRAME_LOG`）。
    enabled: bool,
    /// 本轮统计中第一帧的时刻。
    since: Option<Instant>,
    /// 本会话累计记录的帧数（仅启用时递增）。
    total_frames: u64,
    /// 本会话累计记录的超时慢帧数（跨上报周期保留，供诊断导出使用）。
    total_slow: u64,
    /// 本会话累计超时帧按主导阶段分类计数。
    total_slow_by_phase: [u64; 4],
    /// 最近 [`SLOW_FRAME_CAP`] 条超时慢帧的环形记录。
    recent_slow: VecDeque<SlowFrameRecord>,
    /// 本轮各帧的总耗时。
    times: Vec<Duration>,
    /// 本轮各帧的解析阶段耗时。
    parse_times: Vec<Duration>,
    /// 本轮各帧的排版阶段耗时。
    layout_times: Vec<Duration>,
    /// 本轮各帧的绘制阶段耗时。
    draw_times: Vec<Duration>,
    /// 本轮各帧的呈现阶段耗时。
    present_times: Vec<Duration>,
    /// 本轮耗时超过一次屏幕刷新间隔的帧数；这些帧必然错过一次刷新。
    slow: usize,
    /// 本轮超时帧按主导阶段（解析、排版、绘制、呈现）分类的计数。
    slow_by_phase: [usize; 4],
    /// 尚未反映到画面上的最早一次鼠标移动的时刻。
    moved: Option<Instant>,
    /// 本轮各次鼠标移动到其后第一帧画完的间隔。
    waits: Vec<Duration>,
    /// 本轮查询光标下的窗口的次数和累计耗时。
    queries: std::cell::Cell<(u32, Duration)>,
    /// 本轮因呈现缓冲都被占用而没有画出的帧数。
    skipped: usize,
    /// 本轮取用时内容已被系统收回的呈现缓冲的块数。
    reclaimed: u32,
}

impl FrameLog {
    /// 按环境变量 `WINC_FRAME_LOG` 创建帧日志；变量未设置时不记录。
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var_os("WINC_FRAME_LOG").is_some(),
            since: None,
            total_frames: 0,
            total_slow: 0,
            total_slow_by_phase: [0; 4],
            recent_slow: VecDeque::new(),
            times: Vec::new(),
            parse_times: Vec::new(),
            layout_times: Vec::new(),
            draw_times: Vec::new(),
            present_times: Vec::new(),
            slow: 0,
            slow_by_phase: [0; 4],
            moved: None,
            waits: Vec::new(),
            queries: std::cell::Cell::new((0, Duration::ZERO)),
            skipped: 0,
            reclaimed: 0,
        }
    }

    /// 导出不包含任何文档正文或路径的本地纯文本诊断摘要（含慢帧主导阶段分布与最近慢帧明细）。
    pub fn diagnostic_report(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "winc diagnostics (local-only, no document content)\n\
             frames={} slow_frames={} slow_phases=parse:{},layout:{},draw:{},present:{} \
             skipped_frames={} reclaimed_surfaces={}",
            self.total_frames,
            self.total_slow,
            self.total_slow_by_phase[FramePhase::Parse as usize],
            self.total_slow_by_phase[FramePhase::Layout as usize],
            self.total_slow_by_phase[FramePhase::Draw as usize],
            self.total_slow_by_phase[FramePhase::Present as usize],
            self.skipped,
            self.reclaimed,
        );
        for rec in &self.recent_slow {
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            let _ = writeln!(
                out,
                "  slow #{}: total={:.2}ms (parse={:.2} layout={:.2} draw={:.2} present={:.2}) \
                 dominant={:?} refresh={:.2}ms size={}x{}",
                rec.frame_index,
                ms(rec.phases.total),
                ms(rec.phases.parse),
                ms(rec.phases.layout),
                ms(rec.phases.draw),
                ms(rec.phases.present),
                rec.dominant,
                ms(rec.refresh),
                rec.size.0,
                rec.size.1,
            );
        }
        out
    }

    /// 记录一次鼠标移动；已有尚未反映到画面上的移动时保留更早的时刻。
    pub fn pointer_moved(&mut self) {
        if self.enabled {
            self.moved.get_or_insert_with(Instant::now);
        }
    }

    /// 执行查询光标下的窗口的 `query`，启用时记录其耗时。
    #[cfg_attr(
        not(any(target_os = "macos", test)),
        allow(dead_code, reason = "仅 macOS 使用")
    )]
    pub fn time_query<T>(&self, query: impl FnOnce() -> T) -> T {
        if !self.enabled {
            return query();
        }
        let started = Instant::now();
        let result = query();
        let (count, total) = self.queries.get();
        self.queries.set((count + 1, total + started.elapsed()));
        result
    }

    /// 记录一帧因呈现缓冲都被占用而没有画出。
    pub fn frame_skipped(&mut self) {
        self.skipped += usize::from(self.enabled);
    }

    /// 是否启用。
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 记录一帧：`phases` 为四阶段与总耗时，`refresh` 为屏幕的刷新间隔，`size` 为画面的物理
    /// 像素尺寸。距本轮第一帧满 [`REPORT_EVERY`] 时输出本轮统计并开始下一轮。
    ///
    /// `reclaimed` 是这一帧取用时内容已被系统收回的呈现缓冲的块数。
    pub fn record(
        &mut self,
        phases: FramePhases,
        refresh: Duration,
        size: (u32, u32),
        reclaimed: u32,
    ) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        self.reclaimed += reclaimed;
        if let Some(moved) = self.moved.take() {
            self.waits.push(now.duration_since(moved));
        }
        let since = *self.since.get_or_insert(now);
        self.total_frames += 1;
        self.times.push(phases.total);
        self.parse_times.push(phases.parse);
        self.layout_times.push(phases.layout);
        self.draw_times.push(phases.draw);
        self.present_times.push(phases.present);
        if phases.total > refresh {
            let dominant = phases.dominant();
            self.slow += 1;
            self.slow_by_phase[dominant as usize] += 1;
            self.total_slow += 1;
            self.total_slow_by_phase[dominant as usize] += 1;
            if self.recent_slow.len() >= SLOW_FRAME_CAP {
                self.recent_slow.pop_front();
            }
            self.recent_slow.push_back(SlowFrameRecord {
                frame_index: self.total_frames,
                phases,
                dominant,
                refresh,
                size,
            });
        }
        if now.duration_since(since) < REPORT_EVERY {
            return;
        }

        self.times.sort_unstable();
        self.parse_times.sort_unstable();
        self.layout_times.sort_unstable();
        self.draw_times.sort_unstable();
        self.present_times.sort_unstable();
        self.waits.sort_unstable();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        // 第 `q` 分位的值（毫秒）；没有记录时为 0。
        let at = |times: &[Duration], q: f64| {
            let index = (times.len().saturating_sub(1) as f64 * q) as usize;
            times.get(index).map_or(0.0, |&d| ms(d))
        };
        let (queries, query_time) = self.queries.take();
        eprintln!(
            "winc frame-log: frames={} draw_p50_ms={:.2} draw_p95_ms={:.2} draw_max_ms={:.2} \
             parse_p95_ms={:.2} layout_p95_ms={:.2} paint_p95_ms={:.2} present_p95_ms={:.2} \
             slow_frames={} slow_phases=parse:{},layout:{},draw:{},present:{} \
             refresh_ms={:.2} size={}x{} pointer_moves={} \
             pointer_to_frame_p50_ms={:.2} pointer_to_frame_p95_ms={:.2} \
             pointer_to_frame_max_ms={:.2} window_queries={} window_query_total_ms={:.2} \
             skipped_frames={} reclaimed_surfaces={}",
            self.times.len(),
            at(&self.times, 0.5),
            at(&self.times, 0.95),
            at(&self.times, 1.0),
            at(&self.parse_times, 0.95),
            at(&self.layout_times, 0.95),
            at(&self.draw_times, 0.95),
            at(&self.present_times, 0.95),
            self.slow,
            self.slow_by_phase[FramePhase::Parse as usize],
            self.slow_by_phase[FramePhase::Layout as usize],
            self.slow_by_phase[FramePhase::Draw as usize],
            self.slow_by_phase[FramePhase::Present as usize],
            ms(refresh),
            size.0,
            size.1,
            self.waits.len(),
            at(&self.waits, 0.5),
            at(&self.waits, 0.95),
            at(&self.waits, 1.0),
            queries,
            ms(query_time),
            self.skipped,
            self.reclaimed,
        );
        self.since = None;
        self.times.clear();
        self.parse_times.clear();
        self.layout_times.clear();
        self.draw_times.clear();
        self.present_times.clear();
        self.waits.clear();
        (self.slow, self.skipped, self.reclaimed) = (0, 0, 0);
        self.slow_by_phase = [0; 4];
    }
}

impl Drop for FrameLog {
    fn drop(&mut self) {
        if self.enabled && self.total_slow > 0 {
            eprint!("{}", self.diagnostic_report());
        }
    }
}

#[cfg(test)]
mod tests;
