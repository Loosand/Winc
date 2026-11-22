//! Ported from Quiet MD (`home_folder.rs`, `sidebar_drag.rs`, revision e59c49a).
//! Copyright (c) 2026 s1dashu, MIT; see the repository NOTICE.

use std::time::Instant;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
