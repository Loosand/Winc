//! `frame_log` 的测试：未启用时不记录，启用时累计帧数、分阶段耗时和超时帧主导阶段。

use super::*;

#[test]
fn frames_are_counted_only_when_enabled() {
    let ms = Duration::from_millis;
    let mut off = FrameLog {
        enabled: false,
        ..FrameLog::from_env()
    };
    off.record(FramePhases::total_only(ms(30)), ms(16), (100, 100), 0);
    assert!(off.times.is_empty());

    let mut on = FrameLog {
        enabled: true,
        ..FrameLog::from_env()
    };
    // 一帧在刷新间隔以内，一帧超过（且排版为主导阶段）：只有后者计入超时帧及对应阶段。
    on.record(FramePhases::total_only(ms(5)), ms(16), (100, 100), 0);
    on.record(
        FramePhases {
            parse: ms(2),
            layout: ms(18),
            draw: ms(7),
            present: ms(3),
            total: ms(30),
        },
        ms(16),
        (100, 100),
        0,
    );
    assert_eq!(on.times.len(), 2);
    assert_eq!(on.slow, 1);
    assert_eq!(on.slow_by_phase, [0, 1, 0, 0]);
}

#[test]
fn phase_meter_decomposes_parse_layout_draw_and_present_without_double_counting() {
    let ms = Duration::from_millis;
    let mut meter = FramePhaseMeter::default();
    meter.begin(true);
    // 绘制回调之前的 UI 排版与同步解析。
    meter.add_layout(ms(1), false);
    meter.add_parse(ms(2), false);
    // 绘制回调内部的 Markdown 排版。
    meter.add_layout(ms(4), true);
    // `draw_wall` 为 10ms（含 4ms 回调内排版 + 1ms 呈现 -> 纯绘制应为 5ms）。
    let phases = meter.finish(ms(10), ms(1));
    assert_eq!(phases.parse, ms(2));
    assert_eq!(phases.layout, ms(5));
    assert_eq!(phases.draw, ms(5));
    assert_eq!(phases.present, ms(1));
    assert_eq!(phases.total, ms(13));
    assert_eq!(phases.dominant(), FramePhase::Layout);

    // 关闭时直接透传 `draw_wall`、不记录任何细分阶段。
    meter.begin(false);
    meter.add_parse(ms(9), false);
    let off = meter.finish(ms(6), ms(2));
    assert_eq!(off.parse, Duration::ZERO);
    assert_eq!(off.layout, Duration::ZERO);
    assert_eq!(off.draw, ms(6));
    assert_eq!(off.present, Duration::ZERO);
    assert_eq!(off.total, ms(6));
}

#[test]
fn pointer_moves_are_timed_until_the_next_frame() {
    let ms = Duration::from_millis;
    let mut log = FrameLog {
        enabled: true,
        ..FrameLog::from_env()
    };
    // 一帧之前的两次移动只记一次，从较早的一次算起；没有移动的帧不记间隔。
    log.pointer_moved();
    let first = log.moved;
    log.pointer_moved();
    assert_eq!(log.moved, first);
    assert_eq!(log.time_query(|| 7), 7);
    log.frame_skipped();
    log.record(FramePhases::total_only(ms(5)), ms(16), (100, 100), 2);
    log.record(FramePhases::total_only(ms(5)), ms(16), (100, 100), 0);
    assert_eq!((log.waits.len(), log.moved), (1, None));
    assert_eq!((log.queries.get().0, log.skipped, log.reclaimed), (1, 1, 2));

    // 未启用时都不记录，查询照常执行。
    let mut off = FrameLog {
        enabled: false,
        ..FrameLog::from_env()
    };
    off.pointer_moved();
    off.frame_skipped();
    assert_eq!(off.time_query(|| 7), 7);
    assert_eq!((off.moved, off.skipped, off.queries.get().0), (None, 0, 0));
}
