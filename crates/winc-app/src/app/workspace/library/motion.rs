//! Ported from Quiet MD (`home_folder.rs`, `sidebar_drag.rs`, revision e59c49a).
//! Copyright (c) 2026 s1dashu, MIT; see the repository NOTICE.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{AttachmentPreview, Card};
use crate::app::animation::{
    Clock, LIBRARY_ENTER_ANIMATION, LIBRARY_EXIT_ANIMATION, LIBRARY_SWITCH_ANIMATION, stage,
};

/// Quiet MD's scalar damped spring. Four substeps keep its rhythm at different refresh rates.
#[derive(Clone, Copy)]
struct Spring {
    p: f32,
    v: f32,
    to: f32,
    k: f32,
    c: f32,
}

impl Spring {
    const fn new(p: f32, k: f32, c: f32) -> Self {
        Self {
            p,
            v: 0.0,
            to: p,
            k,
            c,
        }
    }

    fn step(&mut self, dt: f32) -> bool {
        if self.p == self.to && self.v == 0.0 {
            return false;
        }
        for _ in 0..4 {
            self.v += (self.k * (self.to - self.p) - self.c * self.v) * dt / 4.0;
            self.p += self.v * dt / 4.0;
        }
        if (self.p - self.to).abs() < 0.002 && self.v.abs() < 0.02 {
            self.p = self.to;
            self.v = 0.0;
            false
        } else {
            true
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct FolderPose {
    pub(crate) flap: f32,
    pub(crate) dip: f32,
    pub(crate) papers: [f32; 3],
}

impl FolderPose {
    const OPEN: Self = Self {
        flap: 1.0,
        dip: 0.0,
        papers: [1.0; 3],
    };

    pub(crate) fn lift(self) -> f32 {
        -5.0 * self.flap + 3.0 * self.dip
    }

    /// Quiet MD 的纸张静止位置、展开位移和倾角（仅悬停，不含拖入状态）。
    pub(crate) fn paper(self, i: usize, w: f32, h: f32) -> (f32, f32, f32) {
        let (rest, rise) = [
            ((0.0, 0.16, 0.0_f32), (0.0, -0.07, 0.0)),
            ((0.03, 0.135, 2.5), (0.03, -0.055, 2.0)),
            ((-0.035, 0.115, -3.0), (-0.03, -0.05, -1.5)),
        ][i];
        let p = self.papers[i];
        (
            (rest.0 + rise.0 * p) * w,
            (rest.1 + rise.1 * p) * h + self.lift(),
            (rest.2 + rise.2 * p).to_radians(),
        )
    }
}

struct FolderFx {
    flap: Spring,
    dip: Spring,
    papers: [Spring; 3],
    last: Option<Instant>,
}

impl FolderFx {
    fn new(open: bool) -> Self {
        let p = f32::from(u8::from(open));
        Self {
            flap: Spring::new(p, 260.0, 21.0),
            dip: Spring::new(0.0, 520.0, 15.0),
            papers: [
                Spring::new(p, 320.0, 21.0),
                Spring::new(p, 230.0, 18.0),
                Spring::new(p, 170.0, 15.5),
            ],
            last: None,
        }
    }

    fn advance(&mut self, open: bool, now: Instant) -> bool {
        let dt = self.last.replace(now).map_or(1.0 / 120.0, |last| {
            now.saturating_duration_since(last).as_secs_f32().min(0.032)
        });
        self.step(open, dt)
    }

    fn step(&mut self, open: bool, dt: f32) -> bool {
        let to = f32::from(u8::from(open));
        self.flap.to = to;
        let mut moving = self.flap.step(dt) | self.dip.step(dt);
        for paper in &mut self.papers {
            paper.to = to;
            moving |= paper.step(dt);
        }
        moving
    }

    fn pose(&self) -> FolderPose {
        FolderPose {
            flap: self.flap.p,
            dip: self.dip.p,
            papers: self.papers.map(|p| p.p),
        }
    }
}

/// Winc 调度适配：只推进当前悬停和还没收回的文件夹。
#[derive(Default)]
pub(super) struct FolderMotion {
    hot: Option<usize>,
    affected: Vec<(usize, FolderFx)>,
    moving: bool,
    #[cfg(test)]
    frozen: Option<(usize, FolderPose)>,
}

impl FolderMotion {
    pub(super) fn update(&mut self, hot: Option<usize>, enabled: bool, now: Instant) {
        #[cfg(test)]
        if self.frozen.is_some() {
            return;
        }
        let previous = self.hot;
        self.hot = hot;
        self.moving = false;
        if !enabled {
            self.affected.clear();
            return;
        }
        if let Some(index) = hot
            && !self.affected.iter().any(|(i, _)| *i == index)
        {
            self.affected.push((index, FolderFx::new(previous == hot)));
        }
        self.affected.retain_mut(|(index, fx)| {
            let open = Some(*index) == hot;
            let moving = fx.advance(open, now);
            self.moving |= moving;
            open || moving
        });
    }

    pub(super) fn pose(&self, card: usize) -> FolderPose {
        #[cfg(test)]
        if let Some((_, pose)) = self.frozen.filter(|(index, _)| *index == card) {
            return pose;
        }
        self.affected.iter().find(|(i, _)| *i == card).map_or_else(
            || {
                if self.hot == Some(card) {
                    FolderPose::OPEN
                } else {
                    FolderPose::default()
                }
            },
            |(_, fx)| fx.pose(),
        )
    }

    pub(super) fn press(&mut self, card: usize) {
        if let Some((_, fx)) = self.affected.iter_mut().find(|(i, _)| *i == card) {
            fx.dip.v += 2.2;
        }
    }

    pub(super) fn moving(&self) -> bool {
        self.moving
    }

    #[cfg(test)]
    pub(super) fn freeze(&mut self, card: usize, open: bool, millis: u64) {
        let mut fx = FolderFx::new(!open);
        let dt = 1.0 / 120.0;
        let seconds = millis as f32 / 1000.0;
        let frames = (seconds / dt).floor() as u64;
        for _ in 0..frames {
            fx.step(open, dt);
        }
        let remaining = seconds - frames as f32 * dt;
        if remaining > 0.0 {
            fx.step(open, remaining);
        }
        self.frozen = Some((card, fx.pose()));
    }
}

/// 笔记库目录切换的空间方向，决定焦点缩放、圆角收敛与卡片涌现轨迹。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavDirection {
    /// 深入子目录：旧网格绕被点击文件夹放大淡出，新卡片自该方向舒展落位。
    Enter,
    /// 返回上级目录或主页：当前子目录缩小并收圆角回落至父目录对应文件夹处。
    Exit,
    /// 同级目录切换或从文档切入目录：轻柔交叉淡入上浮。
    Switch,
}

impl NavDirection {
    pub(crate) fn between(old: Option<&Path>, new: &Path) -> Self {
        match old {
            Some(old) if new.starts_with(old) && new != old => Self::Enter,
            Some(old) if old.starts_with(new) && old != new => Self::Exit,
            _ => Self::Switch,
        }
    }

    pub(crate) fn duration(self) -> Duration {
        match self {
            Self::Enter => LIBRARY_ENTER_ANIMATION,
            Self::Exit => LIBRARY_EXIT_ANIMATION,
            Self::Switch => LIBRARY_SWITCH_ANIMATION,
        }
    }
}

/// 切换瞬间暂存的旧目录视图状态：在首帧截取像素快照后立即释放，平时零拷贝开销。
pub(crate) struct OutgoingLibrary {
    pub(crate) folder: Option<PathBuf>,
    pub(crate) cards: Vec<Card>,
    pub(crate) attachments: HashMap<PathBuf, AttachmentPreview>,
    pub(crate) scroll: f32,
    pub(crate) poses: Vec<FolderPose>,
    pub(crate) error: Option<String>,
}

/// 过渡期间旧画面的生命周期。
pub(crate) enum OutgoingLayer {
    /// 首帧待绘制并截取快照的旧目录数据。
    Pending(Box<OutgoingLibrary>),
    /// 已截取的旧目录像素快照，后续各帧直接多线程 SIMD 缩放叠加。
    Captured(winc_render::Snapshot),
    /// 无旧目录画面（例如从笔记文档进入目录）。
    None,
}

/// 过渡期间新目录画面的快照缓存，避免动画每帧重复排版折行与重绘所有卡片。
pub(crate) enum IncomingLayer {
    /// 尚未截取，或目录卡片/摘要刚有更新需要刷新一次快照。
    Dirty,
    /// 已截取的新目录像素快照（无悬停高亮框）及其对应的滚动位置。
    Captured {
        snap: winc_render::Snapshot,
        scroll: f32,
    },
}

/// 单个快照图层在当前帧的变换与淡入淡出参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LayerTransform {
    pub(crate) pivot: (f32, f32),
    pub(crate) offset: (f32, f32),
    pub(crate) scale: f32,
    pub(crate) radius: f32,
    pub(crate) opacity: f32,
}

/// 过渡动画时钟：先短暂等待首屏目录摘要就绪再正式起表，杜绝首帧跳过数十毫秒的卡顿感。
enum TransitionClock {
    Waiting { since: Instant, duration: Duration },
    Running(Clock),
}

/// 最多等待本地目录首批真实摘要的时长；超时后立刻起表展示骨架淡入，避免慢盘阻塞。
const READY_WAIT_CAP: Duration = Duration::from_millis(50);

/// 笔记库进入/退出/切换文件夹的双快照焦点缩放过渡状态。
pub(crate) struct FolderTransition {
    pub(crate) direction: NavDirection,
    clock: TransitionClock,
    /// 焦点锚点（窗口逻辑坐标）：`Enter` 为被点文件夹中心，`Exit` 在父目录就绪后解析。
    pub(crate) origin: Option<(f32, f32)>,
    /// 离开的目录路径，供 `Exit` 在父目录卡片网格中定位对应的目标文件夹卡片。
    pub(crate) from_folder: Option<PathBuf>,
    pub(crate) outgoing: OutgoingLayer,
    pub(crate) incoming: IncomingLayer,
}

fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

/// 临界阻尼弹簧曲线（无超调、起步 C1 柔和、收尾轻盈）：`1 - (1 + ωt) e^(-ωt)` 并归一化到 `[0, 1]`。
fn spring_curve(t: f32, omega: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let end = 1.0 - (1.0 + omega) * (-omega).exp();
    ((1.0 - (1.0 + omega * t) * (-omega * t).exp()) / end).clamp(0.0, 1.0)
}

impl FolderTransition {
    pub(crate) fn new_waiting(
        direction: NavDirection,
        origin: Option<(f32, f32)>,
        from_folder: Option<PathBuf>,
        outgoing: OutgoingLayer,
    ) -> Self {
        Self {
            direction,
            clock: TransitionClock::Waiting {
                since: Instant::now(),
                duration: direction.duration(),
            },
            origin,
            from_folder,
            outgoing,
            incoming: IncomingLayer::Dirty,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_running(
        direction: NavDirection,
        clock: Clock,
        origin: Option<(f32, f32)>,
        from_folder: Option<PathBuf>,
        outgoing: OutgoingLayer,
    ) -> Self {
        Self {
            direction,
            clock: TransitionClock::Running(clock),
            origin,
            from_folder,
            outgoing,
            incoming: IncomingLayer::Dirty,
        }
    }

    pub(crate) fn is_waiting(&self) -> bool {
        matches!(self.clock, TransitionClock::Waiting { .. })
    }

    pub(crate) fn wait_expired(&self) -> bool {
        match self.clock {
            TransitionClock::Waiting { since, .. } => since.elapsed() >= READY_WAIT_CAP,
            TransitionClock::Running(_) => true,
        }
    }

    /// 首批真实卡片就绪（或超过等待上限）时正式启动动画时钟，使第一动画帧严格从 `t = 0` 起步。
    pub(crate) fn start_now(&mut self) {
        if let TransitionClock::Waiting { duration, .. } = self.clock {
            self.clock = TransitionClock::Running(Clock::new(duration));
        }
    }

    /// 目录卡片或批次摘要更新时标记新快照脏，下帧重捕一次真实内容。
    pub(crate) fn invalidate_incoming(&mut self) {
        self.incoming = IncomingLayer::Dirty;
    }

    pub(crate) fn done(&self) -> bool {
        match self.clock {
            TransitionClock::Waiting { .. } => false,
            TransitionClock::Running(clock) => clock.done(),
        }
    }

    fn linear(&self) -> f32 {
        match self.clock {
            TransitionClock::Waiting { .. } => 0.0,
            TransitionClock::Running(clock) => clock.linear(),
        }
    }

    fn has_outgoing(&self) -> bool {
        !matches!(self.outgoing, OutgoingLayer::None)
    }

    /// 旧目录快照在当前帧的变换；在前 36% 利落淡出，避免与新目录正文双影糊成一片。
    pub(crate) fn outgoing_style(&self, anchor: (f32, f32)) -> Option<LayerTransform> {
        let t = self.linear();
        let fade_end = 0.36;
        if t >= fade_end {
            return None;
        }
        let u = (t / fade_end).clamp(0.0, 1.0);
        let ease = ease_out_cubic(u);
        let opacity = 1.0 - stage(t, 0.0, fade_end);
        let (scale, offset, radius) = match self.direction {
            NavDirection::Enter => (1.0 + 0.14 * ease, (0.0, 0.0), 0.0),
            NavDirection::Exit => (1.0 - 0.12 * ease, (0.0, 0.0), 18.0 * ease),
            NavDirection::Switch => (1.0 - 0.03 * ease, (0.0, -10.0 * ease), 0.0),
        };
        Some(LayerTransform {
            pivot: anchor,
            offset,
            scale,
            radius,
            opacity,
        })
    }

    /// 新目录快照在当前帧的变换：与旧画面保持同向镜头纵深动势，借助临界阻尼弹簧丝滑减速落位。
    pub(crate) fn incoming_style(&self, anchor: (f32, f32)) -> Option<LayerTransform> {
        let t = self.linear();
        let opacity = if self.has_outgoing() {
            stage(t, 0.10, 0.44)
        } else {
            stage(t, 0.0, 0.40)
        };
        if opacity <= 0.005 {
            return None;
        }
        let s = spring_curve(t, 7.5);
        let (scale, offset, radius) = match self.direction {
            NavDirection::Enter => (0.90 + 0.10 * s, (0.0, 0.0), 16.0 * (1.0 - s)),
            NavDirection::Exit => (1.09 - 0.09 * s, (0.0, 0.0), 0.0),
            NavDirection::Switch => (0.96 + 0.04 * s, (0.0, 14.0 * (1.0 - s)), 10.0 * (1.0 - s)),
        };
        Some(LayerTransform {
            pivot: anchor,
            offset,
            scale,
            radius,
            opacity,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn folder_transition_curves_zoom_anchor_and_settle_cleanly() {
        let anchor = (200.0, 180.0);

        // Enter: t=0 只有旧画面 100% 像素级无缝衔接；新画面在旧画面开始消退后同向放大浮现。
        let start = FolderTransition::new_running(
            NavDirection::Enter,
            Clock::stopped(LIBRARY_ENTER_ANIMATION, Duration::ZERO),
            Some(anchor),
            None,
            OutgoingLayer::Captured(winc_render::Snapshot::default()),
        );
        let out0 = start.outgoing_style(anchor).unwrap();
        assert!((out0.scale - 1.0).abs() < 1e-4);
        assert!((out0.opacity - 1.0).abs() < 1e-4);
        assert!(start.incoming_style(anchor).is_none());

        // t=22%：旧画面已大幅淡出并向焦点放大，新画面同向放大浮现，无双黑字糊成一片。
        let mid = FolderTransition::new_running(
            NavDirection::Enter,
            Clock::stopped(LIBRARY_ENTER_ANIMATION, Duration::from_millis(80)),
            Some(anchor),
            None,
            OutgoingLayer::Captured(winc_render::Snapshot::default()),
        );
        let out_mid = mid.outgoing_style(anchor).unwrap();
        let in_mid = mid.incoming_style(anchor).unwrap();
        assert!(out_mid.scale > 1.08 && out_mid.opacity < 0.45);
        assert!(in_mid.scale > 0.90 && in_mid.scale < 0.99 && in_mid.opacity > 0.15);

        // t=60%：旧画面彻底消失，新画面全不透明（走直写快路径）继续丝滑减速至 1.0×。
        let tail = FolderTransition::new_running(
            NavDirection::Enter,
            Clock::stopped(LIBRARY_ENTER_ANIMATION, Duration::from_millis(216)),
            Some(anchor),
            None,
            OutgoingLayer::Captured(winc_render::Snapshot::default()),
        );
        assert!(tail.outgoing_style(anchor).is_none());
        let in_tail = tail.incoming_style(anchor).unwrap();
        assert!(
            (in_tail.opacity - 1.0).abs() < 1e-4 && in_tail.scale > 0.98 && in_tail.scale < 1.0
        );

        // t=100%：完整落位并结束。
        let end = FolderTransition::new_running(
            NavDirection::Enter,
            Clock::stopped(LIBRARY_ENTER_ANIMATION, LIBRARY_ENTER_ANIMATION),
            Some(anchor),
            None,
            OutgoingLayer::None,
        );
        assert!(end.done());
        assert!(end.outgoing_style(anchor).is_none());
        let in_end = end.incoming_style(anchor).unwrap();
        assert!((in_end.scale - 1.0).abs() < 1e-4 && (in_end.opacity - 1.0).abs() < 1e-4);

        // Exit: 旧画面缩小并收圆角回落，新父目录同向从放大态回落至 1.0×。
        let exit_mid = FolderTransition::new_running(
            NavDirection::Exit,
            Clock::stopped(LIBRARY_EXIT_ANIMATION, Duration::from_millis(75)),
            None,
            Some(PathBuf::from("/notes/blog")),
            OutgoingLayer::Captured(winc_render::Snapshot::default()),
        );
        let out_exit = exit_mid.outgoing_style(anchor).unwrap();
        let in_exit = exit_mid.incoming_style(anchor).unwrap();
        assert!(out_exit.scale < 0.95 && out_exit.radius > 6.0);
        assert!(in_exit.scale > 1.01 && in_exit.scale < 1.09);
    }

    #[test]
    fn nav_direction_distinguishes_enter_exit_and_switch() {
        let root = Path::new("/notes");
        let blog = Path::new("/notes/blog");
        let work = Path::new("/notes/work");
        assert_eq!(NavDirection::between(Some(root), blog), NavDirection::Enter);
        assert_eq!(NavDirection::between(Some(blog), root), NavDirection::Exit);
        assert_eq!(
            NavDirection::between(Some(blog), work),
            NavDirection::Switch
        );
        assert_eq!(NavDirection::between(None, root), NavDirection::Switch);
    }

    #[test]
    fn a_hovered_stack_keeps_its_momentum_on_exit_and_settles() {
        let mut motion = FolderMotion::default();
        let start = Instant::now();
        for f in 0..7 {
            motion.update(Some(3), true, start + Duration::from_millis(f * 8));
        }
        let p = motion.pose(3);
        assert!(motion.moving() && p.papers[0] > p.papers[1] && p.papers[1] > p.papers[2]);
        assert_eq!(motion.pose(2), FolderPose::default());
        motion.update(None, true, start + Duration::from_millis(48));
        assert_eq!(motion.pose(3), p, "转向时不跳变");
        motion.update(None, true, start + Duration::from_millis(56));
        assert!(
            motion.pose(3).papers[0] > p.papers[0],
            "移走时仍有向上的惯性"
        );
        for f in 8..240 {
            motion.update(None, true, start + Duration::from_millis(f * 8));
        }
        assert_eq!(motion.pose(3), FolderPose::default());
        assert!(!motion.moving() && motion.affected.is_empty());
    }

    #[test]
    fn motion_disabled_changes_pose_without_scheduling_frames() {
        let mut motion = FolderMotion::default();
        motion.update(Some(0), false, Instant::now());
        assert_eq!(motion.pose(0), FolderPose::OPEN);
        assert!(!motion.moving());
        motion.update(None, false, Instant::now());
        assert_eq!(motion.pose(0), FolderPose::default());
    }
}
