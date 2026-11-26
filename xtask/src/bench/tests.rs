//! 基准输出的解析测试：`footprint` 的单位换算与 `WINC` 字段的提取。

use super::*;

#[test]
fn parses_footprint_units() {
    assert_eq!(
        parse_footprint("winc [1]: 64-bit    Footprint: 49 MB (16384 bytes per page)"),
        Some(49.0)
    );
    assert_eq!(
        parse_footprint("x Footprint: 512 KB (16384 bytes per page)"),
        Some(0.5)
    );
    assert_eq!(parse_footprint("nothing here"), None);
}

#[test]
fn parses_bench_fields() {
    let out = "WINC main_entry_epoch_ms=10.5\nWINC scroll_frames=300 p50_ms=1.59 p95_ms=2.57 max_ms=3.49 parse_p95_ms=0.12 layout_p95_ms=0.25 draw_p95_ms=2.01 present_p95_ms=0.19\n";
    assert_eq!(field(out, "main_entry_epoch_ms"), Some(10.5));
    assert_eq!(field(out, "p95_ms"), Some(2.57));
    assert_eq!(field(out, "missing"), None);

    let stats = FrameStats::parse(out).expect("应解析分阶段帧耗时");
    assert_eq!(stats.p50, 1.59);
    assert_eq!(stats.p95, 2.57);
    assert_eq!(stats.parse_p95, Some(0.12));
    assert_eq!(stats.layout_p95, Some(0.25));
    assert_eq!(stats.draw_p95, Some(2.01));
    assert_eq!(stats.present_p95, Some(0.19));
}
