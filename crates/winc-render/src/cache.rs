//! 渲染层共用的缓存工具：小整数键的快速哈希和按代淘汰的缓存。
//!
//! 缓存都以帧为失效周期：字形与描边蒙版在帧首检查占用，超过上限时淘汰旧的一代。

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::{BuildHasherDefault, Hash, Hasher};

/// 适用于小整数键的快速哈希，算法取自 rustc 内部的 `FxHasher`。
///
/// 键都是字形 id、码点、字号这样的小整数，不来自外部输入，不需要标准库默认 `SipHash` 的
/// 防碰撞攻击能力；绘制每个字符都要查几次表，哈希开销直接算在每帧的时间预算里。
#[derive(Default, Clone, Copy)]
pub(crate) struct FxHasher {
    hash: u64,
}

impl FxHasher {
    /// 每次写入乘的常数，单位无量纲，取自 rustc 的 `FxHasher`（黄金分割比的小数部分左移 64 位）。
    const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(Self::SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for &chunk in chunks {
            self.add(u64::from_le_bytes(chunk));
        }
        for &b in rest {
            self.add(u64::from(b));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

/// 用 [`FxHasher`] 的哈希表。
pub(crate) type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

/// 按代淘汰的缓存：新一代的估算占用超过 `cap` 字节后，新一代整代降为旧的一代，旧的一代释放，
/// 旧一代里再次用到的项搬回新一代。
///
/// 分两代是为了让刚用过的项留在缓存里，最多再搬一次，总占用最多约为 `cap` 的两倍；整体清空的
/// 缓存在一帧要用的项多于上限时，每帧都要重新生成全部项。按帧换代时在帧首换代，帧中间只在
/// 超过 `FRAME_SLACK` 倍上限时强制换代，避免丢弃本帧还要用的项。
pub(crate) struct Generations<K, V> {
    /// 新的一代。
    new: FastMap<K, V>,
    /// 旧的一代：下一次换代时释放。
    old: FastMap<K, V>,
    /// `new` 估算占用的字节数。
    bytes: usize,
    /// 一代的软上限（字节），即 `new` 的估算占用超过该值时在帧首换代。
    cap: usize,
    /// 帧中间（`get_or_insert_with` 里）的换代上限（字节）：不按帧换代时等于 `cap`。
    hard: usize,
}

/// 按帧换代时，一帧之中新一代的估算占用最多达到 `cap` 的倍数。
///
/// 取 4 是经验值：一帧要用的项通常不超过上限的 2 倍，留出一倍余量后仍能在帧中间强制换代，
/// 防止一次性绘制异常多的项时内存无上限增长。
const FRAME_SLACK: usize = 4;

impl<K: Hash + Eq + Copy, V> Generations<K, V> {
    /// 创建一代上限为 `cap` 字节的空缓存。
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            new: FastMap::default(),
            old: FastMap::default(),
            bytes: 0,
            cap,
            hard: cap,
        }
    }

    /// 创建按帧换代的空缓存：一代上限为 `cap` 字节，帧首调用 [`Generations::new_frame`]。
    pub(crate) fn per_frame(cap: usize) -> Self {
        Self {
            hard: cap * FRAME_SLACK,
            ..Self::new(cap)
        }
    }

    /// 开始新的一帧：新一代的估算占用超过 `cap` 时换代。
    pub(crate) fn new_frame(&mut self) {
        if self.bytes > self.cap {
            self.swap();
        }
    }

    fn swap(&mut self) {
        self.old = std::mem::take(&mut self.new);
        self.bytes = 0;
    }

    /// 清空两代，占用估算归零。
    pub(crate) fn clear(&mut self) {
        self.new = FastMap::default();
        self.old = FastMap::default();
        self.bytes = 0;
    }

    /// 返回 `key` 对应的可变引用；两代里都没有时用 `make` 生成并放进新一代。
    ///
    /// `cost` 返回一项占用的字节数，用于估算新一代的占用。新一代超过上限时在本次调用开头换代，
    /// 本次返回的项一定留在缓存里。
    pub(crate) fn get_or_insert_with(
        &mut self,
        key: K,
        cost: impl FnOnce(&V) -> usize,
        make: impl FnOnce() -> V,
    ) -> &mut V {
        if self.bytes > self.hard {
            self.swap();
        }
        match self.new.entry(key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let value = self.old.remove(&key).unwrap_or_else(make);
                self.bytes += cost(&value);
                entry.insert(value)
            }
        }
    }
}

#[cfg(test)]
mod tests;
