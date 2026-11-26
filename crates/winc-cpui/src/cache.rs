//! 静态区域像素快照缓存：在浮层或高频侧栏更新时直接恢复下层画面，跳过静态主区的 CPU 重绘。
//!
//! 内部复用同一块 [`winc_render::Snapshot`] 缓冲（经 [`Frame::snapshot_into`] 写入），避免高频
//! 捕获时反复向系统分配与释放数 MB 大块内存；当浮层关闭或不再需要缓存时调用 [`SurfaceCache::clear`]
//! 立即归还内存，守住进程常驻内存上限。

use winc_render::{Color, Frame, Rect, Snapshot};

/// 带版本键与几何校验的区域像素快照缓存。
///
/// 只有当缓存处于有效态、版本键 `K` 相等、且快照覆盖的逻辑区域与缩放倍数与当前帧完全一致时
/// 才允许复用；尺寸或缩放变化会自动失效。
#[derive(Debug)]
pub struct SurfaceCache<K> {
    snapshot: Snapshot,
    key: Option<K>,
    valid: bool,
    reuses: u64,
}

impl<K> Default for SurfaceCache<K> {
    fn default() -> Self {
        Self {
            snapshot: Snapshot::default(),
            key: None,
            valid: false,
            reuses: 0,
        }
    }
}

impl<K: PartialEq> SurfaceCache<K> {
    /// 缓存是否仍覆盖当前缩放下的 `rect` 且版本键匹配 `key`。
    pub fn is_fresh(&self, rect: Rect, scale: f32, key: &K) -> bool {
        self.valid && self.key.as_ref() == Some(key) && self.snapshot.covers(rect, scale)
    }

    /// 捕获 `rect` 当前已绘制的像素，并记录版本键 `key`（复用已分配的快照缓冲）。
    pub fn capture(&mut self, frame: &Frame<'_>, rect: Rect, key: K) {
        frame.snapshot_into(rect, &mut self.snapshot);
        self.key = Some(key);
        self.valid = true;
    }

    /// 若缓存新鲜，将快照按 `tint` 恢复到 `frame` 并返回 `true`（同时累计命中次数）。
    ///
    /// `tint` 全透明时原样恢复，不做额外色彩叠加。
    pub fn restore(&mut self, frame: &mut Frame<'_>, rect: Rect, key: &K, tint: Color) -> bool {
        if !self.is_fresh(rect, frame.scale(), key) {
            return false;
        }
        frame.restore_tinted(&self.snapshot, tint);
        self.reuses += 1;
        true
    }

    /// 标记缓存失效（保留已分配像素内存，供下次 `capture` 原地复用）。
    pub fn invalidate(&mut self) {
        self.valid = false;
    }

    /// 清空并释放像素缓冲，立即归还系统内存（不计入空闲常驻内存）。
    pub fn clear(&mut self) {
        self.valid = false;
        self.key = None;
        self.snapshot = Snapshot::default();
    }

    /// 返回累计通过快照复用跳过下层重绘的帧数。
    pub fn reuses(&self) -> u64 {
        self.reuses
    }

    /// 是否持有有效快照（用于诊断与测试检查捕获/释放状态）。
    pub fn is_valid(&self) -> bool {
        self.valid
    }
}
