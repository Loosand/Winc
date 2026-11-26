//! Minc 富文本编辑器内核。
//!
//! 以 Markdown 原文为唯一数据源，按蓝图划分为清晰的层次边界，不依赖窗口系统与顶层应用：
//! - [`mapping`]：文本与坐标层（位置映射 [`ChangeMap`]、坐标桥 [`SourceBridge`]、围栏与行边界查询）；
//! - [`decorations`]：非侵入式装饰层（[`DecorationSet`]、区间标记、行内虚文字与块级虚拟区块）；
//! - [`layout`]：排版与几何层（[`Laid`] 整篇块级缓存排版与视图坐标 ↔ 原文位置双向定位）；
//! - [`session`]：编辑会话状态与可见光标导航（[`LiveState`]、隐藏标记跳过、折行归属与竖直导航）；
//! - [`commands`]：无副作用的结构化编辑命令、按键路由与写作命令规划器；
//! - [`input_rules`]：可单步撤销的两段式输入规则引擎（[`InputRulePlan`]）；
//! - [`clipboard`]：隐藏标记选区扩展、富文本复制载荷与智能粘贴改写规划器。

pub mod clipboard;
pub mod commands;
pub mod decorations;
pub mod input_rules;
pub mod layout;
pub mod mapping;
pub mod session;

pub use clipboard::{CopyPayload, PasteSpot};
pub use commands::{
    BackspaceAction, DeleteAction, EditPlan, EnterPlan, LiveKey, LiveKeyAction, LiveKeyModifiers,
    TableStepPlan, WritingCommand,
};
pub use decorations::{
    Decoration, DecorationId, DecorationKind, DecorationLayer, DecorationSet, MarkStyle,
    WidgetStyle,
};
pub use input_rules::{FenceEnterRule, InputRulePlan};
pub use layout::{
    BASE_RATIO, Before, COPY, DETAILS, EXPAND, Fading, Key, LAYOUT_BUDGET, Laid, LaidWidget, Later,
    LayoutInput, Leaving, LiveKey as LayoutLiveKey, LiveLeaf, MAX_WIDTH, Memo, Moves, NEAR_BLOCKS,
    Near, PAD, PAD_NARROW, Point, SHED_BLOCKS, TABLE, TASK, WIDE_WIDTH, WIDGET_GAP, interpolate,
    line_box,
};
pub use mapping::{
    Assoc, ChangeMap, ChangeSeq, Fenced, SourceBridge, fenced_lines, in_fence, is_image_url,
    is_url, leaf_at_or_before, line_end, line_text, paragraph_lines,
};
pub use session::{LiveState, row_affinity, vertical_move_target, visible_position};
