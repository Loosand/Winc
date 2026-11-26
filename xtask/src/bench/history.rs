//! 基准结果的历史落盘（JSONL）与跨构建趋势对比。
//!
//! 每条记录以单行 JSON 追加到本地历史文件（默认 `target/bench-history.jsonl`），不依赖额外
//! 第三方序列化库；运行基准时自动与上一条（或 `--compare` 指定的基线）横向对比并标记明显退化。

use std::fmt::Write as _;
use std::fs::{OpenOptions, create_dir_all};
use std::io::Write as _;
use std::path::Path;

/// 触发趋势退化预警的最小相对增幅（百分比）。
const REGRESSION_PCT: f64 = 15.0;

/// 单项基准指标的测量结果。
#[derive(Debug, Clone, PartialEq)]
pub struct MetricSample {
    /// 稳定的机器可读指标键（如 `live_typing_long_md_p95_ms`）。
    pub key: String,
    /// 实测数值。
    pub value: f64,
    /// 指标单位（`ms`、`MB` 等）。
    pub unit: String,
    /// 若为门禁指标则为预算上限；参考指标为 `None`。
    pub budget: Option<f64>,
}

/// 单次构建的完整基准记录。
#[derive(Debug, Clone, PartialEq)]
pub struct BenchRecord {
    /// 构建对应的 git 短提交哈希（不可用时为 `"unknown"`）。
    pub commit: String,
    /// 记录时刻（Unix 秒时间戳）。
    pub timestamp_s: u64,
    /// 本次运行全部门禁指标是否达标。
    pub all_passed: bool,
    /// 本次运行采集的全部指标。
    pub metrics: Vec<MetricSample>,
}

/// 单项指标相对历史基线的变化。
#[derive(Debug, Clone, PartialEq)]
pub struct TrendDiff {
    /// 指标键。
    pub key: String,
    /// 指标单位。
    pub unit: String,
    /// 基线值。
    pub baseline: f64,
    /// 当前值。
    pub current: f64,
    /// 绝对变化量（`current - baseline`）。
    pub delta: f64,
    /// 相对变化百分比。
    pub pct: f64,
    /// 是否达到显著退化阈值。
    pub regressed: bool,
}

impl BenchRecord {
    /// 序列化为单行紧凑 JSON（适合 JSONL 追加写入）。
    pub fn to_json_line(&self) -> String {
        let mut out = String::with_capacity(256 + self.metrics.len() * 64);
        let _ = write!(
            out,
            "{{\"commit\":\"{}\",\"timestamp_s\":{},\"all_passed\":{},\"metrics\":[",
            escape_json(&self.commit),
            self.timestamp_s,
            self.all_passed,
        );
        for (i, m) in self.metrics.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"key\":\"{}\",\"value\":{:.3},\"unit\":\"{}\",\"budget\":",
                escape_json(&m.key),
                m.value,
                escape_json(&m.unit),
            );
            match m.budget {
                Some(b) => {
                    let _ = write!(out, "{b:.3}");
                }
                None => out.push_str("null"),
            }
            out.push('}');
        }
        out.push_str("]}");
        out
    }

    /// 从 [`BenchRecord::to_json_line`] 生成的单行 JSON 解析记录。
    pub fn from_json_line(line: &str) -> Option<Self> {
        let s = line.trim();
        let commit = extract_json_str(s, "\"commit\":\"")?;
        let timestamp_s = extract_json_token(s, "\"timestamp_s\":")?.parse().ok()?;
        let all_passed = extract_json_token(s, "\"all_passed\":")?.parse().ok()?;
        let metrics_start = s.find("\"metrics\":[")? + "\"metrics\":[".len();
        let metrics_end = s.rfind("]}")?;
        if metrics_end < metrics_start {
            return None;
        }
        let body = &s[metrics_start..metrics_end];
        let mut metrics = Vec::new();
        if !body.trim().is_empty() {
            for item in body.split("},{") {
                let item = item.trim_start_matches('{').trim_end_matches('}');
                let key = extract_json_str(item, "\"key\":\"")?;
                let value: f64 = extract_json_token(item, "\"value\":")?.parse().ok()?;
                let unit = extract_json_str(item, "\"unit\":\"")?;
                let budget_tok = extract_json_token(item, "\"budget\":")?;
                let budget = if budget_tok == "null" {
                    None
                } else {
                    Some(budget_tok.parse().ok()?)
                };
                metrics.push(MetricSample {
                    key,
                    value,
                    unit,
                    budget,
                });
            }
        }
        Some(Self {
            commit,
            timestamp_s,
            all_passed,
            metrics,
        })
    }
}

/// 将单条基准记录追加到 JSONL 文件（父目录不存在时自动创建）。
pub fn append_history(path: &Path, record: &BenchRecord) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        create_dir_all(parent).map_err(|e| format!("创建基准历史目录失败：{e}"))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开基准历史文件失败：{e}"))?;
    writeln!(file, "{}", record.to_json_line())
        .map_err(|e| format!("写入基准历史文件失败：{e}"))?;
    Ok(())
}

/// 读取 JSONL 历史文件中最近一条有效记录；文件不存在或为空时返回 `Ok(None)`。
pub fn read_latest(path: &Path) -> Result<Option<BenchRecord>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("读取基准历史文件失败：{e}"))?;
    for line in content.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(record) = BenchRecord::from_json_line(trimmed) {
            return Ok(Some(record));
        }
    }
    Ok(None)
}

/// 对比当前构建与基线记录的同名指标，计算差值、百分比变化与退化标记。
pub fn compare_records(baseline: &BenchRecord, current: &BenchRecord) -> Vec<TrendDiff> {
    let mut diffs = Vec::new();
    for cur in &current.metrics {
        let Some(base) = baseline.metrics.iter().find(|m| m.key == cur.key) else {
            continue;
        };
        let delta = cur.value - base.value;
        let pct = if base.value.abs() > 1e-9 {
            delta / base.value * 100.0
        } else {
            0.0
        };
        let noise_floor = match cur.unit.as_str() {
            "ms" => 0.25,
            "MB" => 2.0,
            _ => 0.1,
        };
        let regressed = pct > REGRESSION_PCT && delta > noise_floor;
        diffs.push(TrendDiff {
            key: cur.key.clone(),
            unit: cur.unit.clone(),
            baseline: base.value,
            current: cur.value,
            delta,
            pct,
            regressed,
        });
    }
    diffs
}

/// 将趋势对比结果输出到标准输出。
pub fn print_comparison(baseline: &BenchRecord, diffs: &[TrendDiff]) {
    if diffs.is_empty() {
        return;
    }
    println!();
    println!(
        "趋势对比（基线提交 {}，时间戳 {}）：",
        baseline.commit, baseline.timestamp_s
    );
    for diff in diffs {
        let flag = if diff.regressed {
            " ⚠ 退化预警"
        } else {
            ""
        };
        println!(
            "  {:<32} {:>7.2} -> {:>7.2}{} ({:+.2}{}, {:+.1}%){flag}",
            diff.key, diff.baseline, diff.current, diff.unit, diff.delta, diff.unit, diff.pct,
        );
    }
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

fn extract_json_str(haystack: &str, prefix: &str) -> Option<String> {
    let start = haystack.find(prefix)? + prefix.len();
    let mut out = String::new();
    let mut chars = haystack[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

fn extract_json_token<'a>(haystack: &'a str, prefix: &str) -> Option<&'a str> {
    let start = haystack.find(prefix)? + prefix.len();
    let rest = &haystack[start..];
    let end = rest.find([',', '}', ']']).unwrap_or(rest.len());
    Some(rest[..end].trim())
}
