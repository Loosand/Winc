//! Minc 富文本编辑器内核。
//!
//! 以 Markdown 原文为唯一数据源，按蓝图划分为清晰的层次边界，不依赖窗口系统与顶层应用：
//! - [`mapping`]：文本与坐标层（位置映射 [`ChangeMap`]、坐标桥 [`SourceBridge`]、围栏与行边界查询）；
//! - [`layout`]：排版与几何层（[`Laid`] 整篇块级缓存排版与视图坐标 ↔ 原文位置双向定位）；
//! - [`session`]：编辑会话状态与可见光标导航（[`LiveState`]、隐藏标记跳过、折行归属与竖直导航）；
//! - [`commands`]：无副作用的结构化编辑命令与智能粘贴规划器（[`EditPlan`]、[`EnterPlan`] 等）。

pub mod commands;
pub mod layout;
pub mod mapping;
pub mod session;

pub use commands::{BackspaceAction, DeleteAction, EditPlan, EnterPlan, PasteSpot, TableStepPlan};
pub use layout::{
    BASE_RATIO, Before, COPY, DETAILS, EXPAND, Fading, Key, LAYOUT_BUDGET, Laid, Later,
    LayoutInput, Leaving, LiveKey, LiveLeaf, MAX_WIDTH, Memo, Moves, NEAR_BLOCKS, Near, PAD,
    PAD_NARROW, Point, SHED_BLOCKS, TABLE, TASK, WIDE_WIDTH, interpolate, line_box,
};
pub use mapping::{
    Assoc, ChangeMap, ChangeSeq, Fenced, SourceBridge, fenced_lines, in_fence, is_image_url,
    is_url, leaf_at_or_before, line_end, line_text, paragraph_lines,
};
pub use session::{LiveState, row_affinity, vertical_move_target, visible_position};
