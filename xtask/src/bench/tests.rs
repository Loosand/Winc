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

#[test]
fn bench_history_round_trips_jsonl_and_flags_regressions() {
    let baseline = BenchRecord {
        commit: "abc1234".to_owned(),
        timestamp_s: 1_700_000_000,
        all_passed: true,
        metrics: vec![
            MetricSample {
                key: "live_typing_long_md_p95_ms".to_owned(),
                value: 2.0,
                unit: "ms".to_owned(),
                budget: Some(4.0),
            },
            MetricSample {
                key: "preview_extreme_md_p95_ms".to_owned(),
                value: 3.0,
                unit: "ms".to_owned(),
                budget: None,
            },
            MetricSample {
                key: "idle_long_md_mb".to_owned(),
                value: 44.0,
                unit: "MB".to_owned(),
                budget: None,
            },
        ],
    };

    let line = baseline.to_json_line();
    let decoded = BenchRecord::from_json_line(&line).expect("JSONL 行应可无损解析");
    assert_eq!(decoded, baseline);

    let dir = std::env::temp_dir().join(format!("winc-bench-history-{}", std::process::id()));
    let path = dir.join("history.jsonl");
    let _ = std::fs::remove_file(&path);
    append_history(&path, &baseline).expect("应成功写入历史文件");

    let current = BenchRecord {
        commit: "def5678".to_owned(),
        timestamp_s: 1_700_000_600,
        all_passed: true,
        metrics: vec![
            // +30% 且 +0.6ms > 噪声门限 -> 触发退化预警。
            MetricSample {
                key: "live_typing_long_md_p95_ms".to_owned(),
                value: 2.6,
                unit: "ms".to_owned(),
                budget: Some(4.0),
            },
            // 小幅改善 -> 不预警。
            MetricSample {
                key: "preview_extreme_md_p95_ms".to_owned(),
                value: 2.7,
                unit: "ms".to_owned(),
                budget: None,
            },
            // +1.0MB < 2MB 噪声门限 -> 不预警。
            MetricSample {
                key: "idle_long_md_mb".to_owned(),
                value: 45.0,
                unit: "MB".to_owned(),
                budget: None,
            },
        ],
    };

    let loaded = read_latest(&path)
        .expect("读取历史文件成功")
        .expect("应读到刚写入的基线");
    assert_eq!(loaded, baseline);
    append_history(&path, &current).expect("应成功追加第二条记录");
    assert_eq!(read_latest(&path).unwrap(), Some(current.clone()));

    let diffs = compare_records(&baseline, &current);
    assert_eq!(diffs.len(), 3);
    assert!(diffs[0].regressed, "超出 +15% 且 >0.25ms 应标记退化");
    assert!(!diffs[1].regressed, "性能改善不应标记退化");
    assert!(!diffs[2].regressed, "低于噪声门限不应误报退化");

    let _ = std::fs::remove_dir_all(&dir);
}
