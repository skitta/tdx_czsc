//! 脱离通达信的算法验证工具：用 CSV 行情数据跑完整分析并输出通道。
//!
//! 用法：
//! ```text
//! cargo run --example analyze_csv -- <csv路径> [min_bi_len] [analysis_window]
//! cargo run --example analyze_csv -- --compare [min_bi_len] [window] <csv路径>...
//! cargo run --example analyze_csv -- ../tests/data/999999_day.csv 5 0
//! ```
//! 窗口参数：0 = 全量（默认），正数 = 取尾部 N 根。
//!
//! 周期由文件名推断：包含 `30m`/`60m` 等视为分钟线，包含 `day`/`日线` 视为日线。

use std::env;
use std::fs;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use prost::Message;

use tdx_czsc_service::analyzer::build_snapshot_profiled;
use tdx_czsc_service::config::AppConfig;
use tdx_czsc_service::proto::analysis::{AnalyzeRequest, CzscOptions, MarketSeries};
use tdx_czsc_service::state::AppState;

const CHANNEL_NAMES: &[(u32, &str)] = &[
    (101, "fx_top"),
    (102, "fx_bottom"),
    (201, "bi_points"),
    (202, "bi_direction"),
    (203, "seg_points"),
    (204, "seg_direction"),
    (301, "zs_upper"),
    (302, "zs_lower"),
    (303, "zs_middle"),
    (304, "zs_boundary"),
    (305, "zs_strict_upper"),
    (306, "zs_strict_lower"),
    (307, "zs_strict_middle"),
    (308, "zs_strict_boundary"),
    (401, "buy1"),
    (402, "sell1"),
    (403, "buy2"),
    (404, "sell2"),
    (405, "buy3"),
    (406, "sell3"),
    (407, "buy3_strict"),
    (408, "sell3_strict"),
];

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        bail!(
            "用法: analyze_csv <csv路径> [min_bi_len] [analysis_window] | --compare [min_bi_len] [window] <csv>..."
        );
    }

    if args[1] == "--compare" {
        return run_compare(&args[2..]);
    }

    let path = &args[1];
    let min_bi_len: usize = args.get(2).map(|s| s.parse().unwrap_or(3)).unwrap_or(3);
    // 0 = 全量（默认）；正数 = 尾部 N 根窗口
    let analysis_window: u32 = args.get(3).map(|s| s.parse().unwrap_or(0)).unwrap_or(0);
    println!("分析窗口: {analysis_window} (0=全量)");
    // 按字节读取：通达信导出文件可能是 GBK 编码，数据行（日期/数字）为 ASCII，
    // 中文表头行会被跳过。
    let raw = fs::read(path).map_err(|e| anyhow!("读取 {path} 失败: {e}"))?;
    let csv = String::from_utf8_lossy(&raw).into_owned();

    let freq_hint = infer_freq_hint(path);
    let (close, series) = parse_csv(&csv)?;
    println!(
        "数据: {}  ({close} 根, freq={freq_hint})",
        path.trim_start_matches("../")
    );

    let config = AppConfig {
        min_bi_len: min_bi_len as u32,
        ..AppConfig::default()
    };
    let derive_volume_proxy = config.derive_volume_proxy;
    let state = Arc::new(AppState::new(config));

    let request = AnalyzeRequest {
        func_group: "single_level_structure".to_string(),
        requested_func_ids: CHANNEL_NAMES.iter().map(|(id, _)| *id).collect(),
        freq_hint,
        data_len: close,
        input_hash: 0xCAFEBABE,
        series: Some(series),
        options: Some(CzscOptions {
            max_bi_num: 1000,
            min_bi_len: min_bi_len as u32,
            analysis_window,
            include_objects: true,
            enable_incremental: false,
        }),
        prefer_cached: false,
    };

    let started = std::time::Instant::now();
    // 拆段计时: 定位分析耗时瓶颈
    let window_start = tdx_czsc_service::analyzer::window_start_of(
        close as usize,
        request.options.as_ref().unwrap().analysis_window,
    );
    let t0 = std::time::Instant::now();
    let bars = tdx_czsc_service::analyzer::build_raw_bars(
        &request,
        tdx_czsc_service::analyzer::parse_freq(&request.freq_hint).expect("freq"),
        window_start,
        derive_volume_proxy,
    )
    .expect("bars");
    let t1 = std::time::Instant::now();
    // 第 1 遍：直连演化剖面（不经 build_snapshot），供诊断"最后一笔详情"；
    // 注意：随后 build_snapshot_profiled 会再跑第 2 遍完整分析——
    // 因此主耗时数字以 build_snapshot_profiled 输出的剖面为准，
    // 本段只用于对照"纯演化"与"快照装配"的差异。
    let (czsc, _points) =
        tdx_czsc_service::analyzer::compute_buy_sell_points(&bars, 1000, min_bi_len);
    let t2 = std::time::Instant::now();
    // 最后一笔详情: 检查 fx_a/fx_b 分型类型与位置
    {
        let finished = czsc.get_finished_bis();
        let raw_last = czsc.bi_list.last();
        println!(
            "笔信息: finished={} bi_list={}",
            finished.len(),
            czsc.bi_list.len()
        );
        if let Some(bi) = finished.last() {
            let a_id = bi.fx_a.elements.get(1).map(|b| b.id);
            let b_id = bi.fx_b.elements.get(1).map(|b| b.id);
            println!(
                "最后一笔(finished): fx_a.mark={:?} id={a_id:?} price={:.2}  fx_b.mark={:?} id={b_id:?} price={:.2}",
                bi.fx_a.mark, bi.fx_a.fx, bi.fx_b.mark, bi.fx_b.fx
            );
        }
        if let Some(bi) = raw_last {
            let a_id = bi.fx_a.elements.get(1).map(|b| b.id);
            let b_id = bi.fx_b.elements.get(1).map(|b| b.id);
            println!(
                "最后一笔(bi_list): fx_a.mark={:?} id={a_id:?} price={:.2}  fx_b.mark={:?} id={b_id:?} price={:.2}",
                bi.fx_a.mark, bi.fx_a.fx, bi.fx_b.mark, bi.fx_b.fx
            );
        }
        // 打印窗口末尾最近 12 笔的日期区间
        let fmt = |dt: chrono::DateTime<chrono::Utc>| dt.format("%Y-%m-%d").to_string();
        let start = finished.len().saturating_sub(12);
        for bi in finished.iter().skip(start) {
            println!(
                "  笔: {:?}→{:?} [{}, {}] price {:.2}→{:.2}",
                bi.fx_a.mark,
                bi.fx_b.mark,
                fmt(bi.fx_a.dt),
                fmt(bi.fx_b.dt),
                bi.fx_a.fx,
                bi.fx_b.fx
            );
        }
    }
    println!(
        "分段: bars={} build_raw_bars={:?} czsc+signals(单遍)={:?}",
        bars.len(),
        t1 - t0,
        t2 - t1
    );
    let params = tdx_czsc_service::analyzer::AnalysisParams::from((&request, &state.config));
    let mut profile = tdx_czsc_service::analyzer::SnapshotProfile::default();
    let snapshot = build_snapshot_profiled(&request, &params, Some(&mut profile))?;
    println!("build_snapshot 总耗时: {:?}", started.elapsed());

    // 阶段剖面（P1）：定位"演化之后"的通道/对象/编码成本
    println!(
        "\n== 阶段剖面 (P1) ==\n\
         rawbars            {:?}\n\
         evolve(单遍)       {:?}\n\
         ├─ update_bar      {:?}\n\
         └─ 信号评估 observe {:?}\n\
         pivots+segs        {:?}\n\
         strict_pivots      {:?}\n\
         channels           {:?}\n\
         objects            {:?}",
        profile.rawbars,
        profile.evolve,
        profile.evolve_update,
        profile.evolve_observe,
        profile.pivots_segs,
        profile.strict_pivots,
        profile.channels,
        profile.objects
    );
    let t_enc = std::time::Instant::now();
    let encoded = snapshot.encode_to_vec();
    println!(
        "protobuf 编码 {:?} (snapshot {} bytes)",
        t_enc.elapsed(),
        encoded.len()
    );

    println!("\n== 通道命中统计 ==");
    for (func_id, name) in CHANNEL_NAMES {
        if let Some(ch) = snapshot.channels.iter().find(|c| c.func_id == *func_id) {
            let hits = ch.values.iter().filter(|v| v.abs() > f32::EPSILON).count();
            println!("{func_id} {name:<12} 非零点: {hits}");
        }
    }

    println!("\n== 买卖点信号 (401~406) ==");
    let signal_hits = snapshot
        .signals
        .iter()
        .filter(|s| !s.label.is_empty())
        .collect::<Vec<_>>();
    if signal_hits.is_empty() {
        println!("(无命中信号)");
    } else {
        for s in &signal_hits {
            println!(
                "  kind={} index={} price={:.2} label={}",
                s.kind, s.index, s.price, s.label
            );
        }
    }
    Ok(())
}

/// --compare：笔中枢 vs 严格段中枢的量化对照，并按 A 验收阈值判定（不达标返回 Err）。
struct CompareRow {
    bi_count: usize,
    strict_count: usize,
    bi_med: f64,
    strict_med: f64,
    bi_narrow: f64,
    strict_narrow: f64,
    strict_virtual: f64,
    strict_per_k: f64,
    signal_hits: usize,
    strict_us: f64,
    ok: bool,
}

fn run_compare(args: &[String]) -> Result<()> {
    let mut nums = Vec::new();
    let mut paths = Vec::new();
    for a in args {
        if paths.is_empty()
            && let Ok(n) = a.parse::<usize>()
        {
            nums.push(n);
            continue;
        }
        paths.push(a.clone());
    }
    if paths.is_empty() {
        bail!("--compare 需要至少一个 csv 路径");
    }
    // 与单文件模式保持一致；计划中的 --compare 命令不带参数。
    let min_bi_len = nums.first().copied().unwrap_or(3);
    let window = nums.get(1).copied().unwrap_or(0) as u32;
    println!("对比 min_bi_len={min_bi_len} window={window}");
    println!(
        "{:<26} {:>4} {:>6} {:>9} {:>9} {:>7} {:>7} {:>7} {:>8} {:>8} {:>10}  结果",
        "file",
        "bi",
        "严L1",
        "bi中位宽",
        "严中位宽",
        "bi窄%",
        "严窄%",
        "严虚%",
        "严/千根",
        "407/408",
        "严耗时us"
    );
    let mut fails = 0;
    let mut signal_hits = 0;
    for path in &paths {
        match compare_one(path, min_bi_len, window) {
            Ok(r) => {
                if !r.ok {
                    fails += 1;
                }
                signal_hits += r.signal_hits;
                println!(
                    "{:<26} {:>4} {:>6} {:>9.3} {:>9.3} {:>7.1} {:>7.1} {:>7.1} {:>8.2} {:>8} {:>10.1}  {}",
                    short_name(path),
                    r.bi_count,
                    r.strict_count,
                    r.bi_med,
                    r.strict_med,
                    r.bi_narrow * 100.0,
                    r.strict_narrow * 100.0,
                    r.strict_virtual * 100.0,
                    r.strict_per_k,
                    r.signal_hits,
                    r.strict_us,
                    if r.ok { "PASS" } else { "FAIL" }
                );
            }
            Err(e) => {
                fails += 1;
                println!("{:<26} 失败: {e}", short_name(path));
            }
        }
    }
    if signal_hits == 0 {
        println!("阶段 B 验收失败：所有文件合计未命中 407/408");
    }
    if fails > 0 || signal_hits == 0 {
        bail!(
            "{fails}/{} 个文件未达标（阈值：严格数≤笔数×60%，严中位宽≥笔×1.5，严窄占比<笔，407/408≥1）",
            paths.len()
        );
    }
    println!("全部达标");
    Ok(())
}

fn compare_one(path: &str, min_bi_len: usize, window: u32) -> Result<CompareRow> {
    let raw = fs::read(path).map_err(|e| anyhow!("读取 {path} 失败: {e}"))?;
    let csv = String::from_utf8_lossy(&raw).into_owned();
    let (len, series) = parse_csv(&csv)?;
    let atr = atr14(&series);
    let request = AnalyzeRequest {
        func_group: "single_level_structure".to_string(),
        requested_func_ids: vec![301, 302, 303, 304, 305, 306, 307, 308],
        freq_hint: infer_freq_hint(path),
        data_len: len,
        input_hash: 0,
        series: Some(series),
        options: Some(CzscOptions {
            max_bi_num: 1000,
            min_bi_len: min_bi_len as u32,
            analysis_window: window,
            include_objects: true,
            enable_incremental: false,
        }),
        prefer_cached: false,
    };
    let config = AppConfig {
        min_bi_len: min_bi_len as u32,
        ..AppConfig::default()
    };
    let params = tdx_czsc_service::analyzer::AnalysisParams::from((&request, &config));
    let mut profile = tdx_czsc_service::analyzer::SnapshotProfile::default();
    let snapshot = build_snapshot_profiled(&request, &params, Some(&mut profile))?;

    let bi_pairs: Vec<(f64, usize)> = snapshot
        .pivots
        .iter()
        .map(|z| (z.upper - z.lower, z.end_index as usize))
        .collect();
    // 量化门槛只比较级别 1 段中枢与笔中枢；级别 2/3 是段中枢的升级/扩张，
    // 与笔中枢不是同一构成单元，计入会虚增数量和窄中枢占比。
    let st_pairs: Vec<(f64, usize)> = snapshot
        .strict_pivots
        .iter()
        .filter(|p| p.level == 1)
        .map(|p| (p.upper - p.lower, p.end_index as usize))
        .collect();
    let bi_widths: Vec<f64> = bi_pairs.iter().map(|(w, _)| *w).collect();
    let st_widths: Vec<f64> = st_pairs.iter().map(|(w, _)| *w).collect();
    let bi_med = median(bi_widths.clone());
    let strict_med = median(st_widths.clone());
    let narrow = |pairs: &[(f64, usize)]| {
        if pairs.is_empty() {
            0.0
        } else {
            let hits = pairs
                .iter()
                .filter(|(w, end)| atr.get(*end).is_some_and(|a| *a > 0.0 && *w < 0.5 * *a))
                .count();
            hits as f64 / pairs.len() as f64
        }
    };
    let (bi_narrow, strict_narrow) = (narrow(&bi_pairs), narrow(&st_pairs));
    let (bi_count, strict_count) = (snapshot.pivots.len(), st_pairs.len());
    let strict_virtual = if strict_count == 0 {
        0.0
    } else {
        snapshot
            .strict_pivots
            .iter()
            .filter(|p| p.level == 1 && !p.is_sure)
            .count() as f64
            / strict_count as f64
    };
    let strict_per_k = strict_count as f64 * 1000.0 / len.max(1) as f64;
    let signal_hits = snapshot
        .signals
        .iter()
        .filter(|s| s.kind == 7 || s.kind == 8)
        .count();
    let ok = bi_count > 0
        && (strict_count as f64) <= bi_count as f64 * 0.6
        && (bi_med <= 0.0 || strict_med >= bi_med * 1.5)
        // 两边都没有窄中枢（0 < 0 不成立）时视为不劣于笔中枢。
        && (strict_narrow < bi_narrow || (strict_narrow == 0.0 && bi_narrow == 0.0));
    Ok(CompareRow {
        bi_count,
        strict_count,
        bi_med,
        strict_med,
        bi_narrow,
        strict_narrow,
        strict_virtual,
        strict_per_k,
        signal_hits,
        strict_us: profile.strict_pivots.as_secs_f64() * 1_000_000.0,
        ok,
    })
}

/// 逐 bar ATR(14)：真实波幅的 14 根简单均值。窄中枢阈值按各自 end_index 取局部值。
fn atr14(series: &MarketSeries) -> Vec<f64> {
    let n = series.close.len();
    let mut tr = vec![0.0; n];
    for (i, value) in tr.iter_mut().enumerate() {
        let prev_close = if i > 0 {
            series.close[i - 1] as f64
        } else {
            series.close[i] as f64
        };
        let high = series.high[i] as f64;
        let low = series.low[i] as f64;
        *value = (high - low)
            .abs()
            .max((high - prev_close).abs())
            .max((low - prev_close).abs());
    }
    let mut atr = vec![0.0; n];
    let mut sum = 0.0;
    for i in 0..n {
        sum += tr[i];
        if i >= 14 {
            sum -= tr[i - 14];
        }
        atr[i] = sum / (i + 1).min(14) as f64;
    }
    atr
}

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

fn short_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// 返回 (bar 数, MarketSeries)
/// 支持两种格式：CSV（逗号分隔）与通达信导出（制表符分隔）。
fn parse_csv(csv: &str) -> Result<(u32, MarketSeries)> {
    let mut series = MarketSeries {
        open: Vec::new(),
        high: Vec::new(),
        low: Vec::new(),
        close: Vec::new(),
        vol: Vec::new(),
        amount: Vec::new(),
        dt_unix_ms: Vec::new(),
    };

    for line in csv.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.contains('(') // 通达信导出: 股票名行
            || line.starts_with("时间")
        // 通达信导出: 表头
        {
            continue;
        }
        let fields: Vec<&str> = if line.contains('\t') {
            line.split('\t').map(str::trim).collect()
        } else {
            line.split(',').map(str::trim).collect()
        };
        if fields.len() < 5 {
            continue;
        }
        let dt = match parse_dt(fields[0]) {
            Ok(dt) => dt,
            Err(_) => continue,
        };
        series.dt_unix_ms.push(dt.and_utc().timestamp_millis());
        series.open.push(
            fields[1]
                .parse::<f32>()
                .map_err(|_| anyhow!("非法数值: {}", fields[1]))?,
        );
        series.high.push(
            fields[2]
                .parse::<f32>()
                .map_err(|_| anyhow!("非法数值: {}", fields[2]))?,
        );
        series.low.push(
            fields[3]
                .parse::<f32>()
                .map_err(|_| anyhow!("非法数值: {}", fields[3]))?,
        );
        series.close.push(
            fields[4]
                .parse::<f32>()
                .map_err(|_| anyhow!("非法数值: {}", fields[4]))?,
        );
        if fields.len() > 5
            && let Ok(v) = fields[5].replace(',', "").parse::<f32>()
        {
            series.vol.push(v);
        }
    }

    let len = series.close.len() as u32;
    if len == 0 {
        bail!("csv 没有数据行");
    }
    Ok((len, series))
}

fn parse_dt(raw: &str) -> Result<NaiveDateTime> {
    if let Ok(dt) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
        return Ok(dt);
    }
    if let Ok(d) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return d
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow!("非法时间: {raw}"));
    }
    if let Ok(d) = NaiveDate::parse_from_str(raw, "%Y/%m/%d") {
        return d
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow!("非法时间: {raw}"));
    }
    Err(anyhow!("无法解析时间: {raw}"))
}

fn infer_freq_hint(path: &str) -> String {
    let lower = path.to_lowercase();
    if lower.contains("day") || lower.contains("日线") || lower.contains("_d.") {
        return "D".to_string();
    }
    for (needle, freq) in [
        ("30m", "F30"),
        ("60m", "F60"),
        ("15m", "F15"),
        ("5m", "F5"),
        ("1m", "F1"),
        ("120m", "F120"),
        ("240m", "F240"),
    ] {
        if lower.contains(needle) {
            return freq.to_string();
        }
    }
    "F30".to_string()
}
