use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Duration, TimeZone, Utc};
use czsc_core::analyze::CZSC;
use czsc_core::objects::bar::RawBarBuilder;
use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction as CzscDirection;
use czsc_core::objects::freq::Freq;
use czsc_core::objects::mark::Mark;
use czsc_core::objects::zs::ZS;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::proto::analysis::{
    AnalysisSnapshot, AnalyzeRequest, ChannelKind, ChannelSeries, FractalMark, FractalPoint,
    PivotZone, Segment as ProtoSegment, SignalPoint, SnapshotMeta, StrokeSegment,
};
use crate::state::AppState;

mod signals;
pub use signals::{BuySellPoint, compute_buy_sell_points};

mod segs;
pub use segs::{Segment, build_segs};

pub fn build_snapshot(request: &AnalyzeRequest, state: &AppState) -> Result<AnalysisSnapshot> {
    let options = request.options.as_ref();
    let data_len = request.data_len as usize;
    let series = request
        .series
        .as_ref()
        .ok_or_else(|| anyhow!("market series is missing"))?;

    validate_series_lengths(request.data_len, series)?;

    let requested_func_ids: Vec<u32> = if request.requested_func_ids.is_empty() {
        vec![101, 102, 201, 301, 302]
    } else {
        request.requested_func_ids.clone()
    };

    let analysis_window = options
        .map(|x| x.analysis_window)
        .filter(|x| *x > 0)
        .unwrap_or(state.config.default_analysis_window);

    let freq = parse_freq(&request.freq_hint)?;
    let window_start = data_len.saturating_sub(analysis_window as usize);
    let bars = build_raw_bars(request, freq, window_start)?;

    if bars.is_empty() {
        bail!("bars are empty after applying analysis window");
    }

    let max_bi_num = options
        .map(|x| x.max_bi_num)
        .filter(|x| *x > 0)
        .unwrap_or(state.config.max_bi_num) as usize;
    let min_bi_len = options
        .map(|x| x.min_bi_len)
        .filter(|x| *x > 0)
        .unwrap_or(state.config.min_bi_len) as usize;

    let wants_signals = requested_func_ids.iter().any(|id| (401..=406).contains(id));
    let mut buy_sell_points = if wants_signals {
        compute_buy_sell_points(&bars, max_bi_num, min_bi_len)
    } else {
        Vec::new()
    };

    let czsc = CZSC::new(bars, max_bi_num, min_bi_len);
    let fractals = czsc.get_fx_list();
    // 已完成的笔（get_finished_bis 已处理"最后一笔待确认"的实时场景：
    // 无包含K线不足 5 根时自动剔除最后一笔）。
    let finished_bis = czsc.get_finished_bis();
    // 缠论规则构建中枢（见 build_pivots：相邻中枢之间必须间隔一笔）。
    let pivots = build_pivots(&finished_bis);
    // 缠论规则构建段（见 segs.rs：特征序列分型法，段 = 至少 3 笔、首尾同向）。
    let segs = build_segs(&finished_bis);
    // 三买/三卖依附中枢严格化：每个中枢最多一个三买或三卖。
    strictize_third_bs_by_zs(&mut buy_sell_points, &pivots);

    let channels = requested_func_ids
        .into_iter()
        .map(|func_id| {
            build_channel(
                func_id,
                data_len,
                &fractals,
                &finished_bis,
                &pivots,
                &segs,
                &buy_sell_points,
            )
        })
        .collect::<Result<Vec<_>>>()?;

    let generated_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow!(e))?
        .as_millis() as u64;

    let include_objects = options.map(|x| x.include_objects).unwrap_or(false);

    Ok(AnalysisSnapshot {
        meta: Some(SnapshotMeta {
            symbol: if request.symbol_hint.is_empty() {
                "TDX".to_string()
            } else {
                request.symbol_hint.clone()
            },
            freq: freq.to_string(),
            data_len: request.data_len,
            input_hash: request.input_hash,
            max_bi_num: max_bi_num as u32,
            min_bi_len: min_bi_len as u32,
            analysis_window,
            generated_at_unix_ms,
            analyzer_version: state.config.czsc_version.clone(),
        }),
        channels,
        fractals: if include_objects {
            fractals.iter().filter_map(map_fractal_point).collect()
        } else {
            Vec::new()
        },
        strokes: if include_objects {
            finished_bis.iter().filter_map(map_stroke_segment).collect()
        } else {
            Vec::new()
        },
        pivots: if include_objects {
            pivots.iter().filter_map(map_pivot_zone).collect()
        } else {
            Vec::new()
        },
        signals: if include_objects {
            buy_sell_points
                .iter()
                .map(|p| SignalPoint {
                    index: p.index as u32,
                    kind: p.kind as i32,
                    price: p.price,
                    label: p.label.clone(),
                })
                .collect()
        } else {
            Vec::new()
        },
        segments: if include_objects {
            segs.iter().filter_map(|s| map_segment(s, &finished_bis)).collect()
        } else {
            Vec::new()
        },
    })
}

fn validate_series_lengths(
    data_len: u32,
    series: &crate::proto::analysis::MarketSeries,
) -> Result<()> {
    let expected = data_len as usize;

    let check = |name: &str, values: &[f32], required: bool| -> Result<()> {
        if values.is_empty() {
            if required {
                bail!("required series {name} is empty");
            }
            return Ok(());
        }
        if values.len() != expected {
            bail!(
                "series {name} length {} != data_len {expected}",
                values.len()
            );
        }
        Ok(())
    };

    check("high", &series.high, true)?;
    check("low", &series.low, true)?;
    check("close", &series.close, true)?;
    check("open", &series.open, false)?;
    check("vol", &series.vol, false)?;
    check("amount", &series.amount, false)?;
    if !series.dt_unix_ms.is_empty() && series.dt_unix_ms.len() != expected {
        bail!(
            "series dt_unix_ms length {} != data_len {expected}",
            series.dt_unix_ms.len()
        );
    }

    Ok(())
}

pub fn parse_freq(freq_hint: &str) -> Result<Freq> {
    if freq_hint.is_empty() {
        return Ok(Freq::F30);
    }

    Freq::from_str(freq_hint)
        .or_else(|_| Freq::from_str(&normalize_freq_hint(freq_hint)))
        .map_err(|_| anyhow!("unsupported freq_hint: {freq_hint}"))
}

fn normalize_freq_hint(freq_hint: &str) -> String {
    match freq_hint {
        "F1" | "1m" => "1分钟".to_string(),
        "F5" | "5m" => "5分钟".to_string(),
        "F15" | "15m" => "15分钟".to_string(),
        "F30" | "30m" => "30分钟".to_string(),
        "F60" | "60m" => "60分钟".to_string(),
        "F120" | "120m" => "120分钟".to_string(),
        "F240" | "240m" => "240分钟".to_string(),
        "d" | "1d" | "D" => "日线".to_string(),
        "w" | "1w" | "W" => "周线".to_string(),
        "m" | "1M" | "M" => "月线".to_string(),
        other => other.to_string(),
    }
}

pub fn build_raw_bars(
    request: &AnalyzeRequest,
    freq: Freq,
    window_start: usize,
) -> Result<Vec<czsc_core::objects::bar::RawBar>> {
    let series = request
        .series
        .as_ref()
        .ok_or_else(|| anyhow!("market series is missing"))?;

    let symbol = if request.symbol_hint.is_empty() {
        "TDX".to_string()
    } else {
        request.symbol_hint.clone()
    };

    let mut bars = Vec::with_capacity(request.data_len as usize - window_start);
    for i in window_start..request.data_len as usize {
        let dt = resolve_bar_datetime(i, &series.dt_unix_ms, freq)?;
        let close = series.close[i] as f64;
        let open = series.open.get(i).copied().unwrap_or(series.close[i]) as f64;
        let vol = series.vol.get(i).copied().unwrap_or(0.0) as f64;
        let amount = series.amount.get(i).copied().unwrap_or(0.0) as f64;

        let bar = RawBarBuilder::default()
            .symbol(symbol.clone())
            .dt(dt)
            .id(i as i32)
            .freq(freq)
            .open(open)
            .close(close)
            .high(series.high[i] as f64)
            .low(series.low[i] as f64)
            .vol(vol)
            .amount(amount)
            .build()
            .map_err(|e| anyhow!("failed to build raw bar at index {i}: {e}"))?;

        bars.push(bar);
    }

    Ok(bars)
}

fn resolve_bar_datetime(index: usize, provided: &[i64], freq: Freq) -> Result<DateTime<Utc>> {
    if let Some(ts_ms) = provided.get(index) {
        return Utc
            .timestamp_millis_opt(*ts_ms)
            .single()
            .ok_or_else(|| anyhow!("invalid dt_unix_ms at index {index}: {ts_ms}"));
    }

    let base = Utc
        .with_ymd_and_hms(2000, 1, 1, 0, 0, 0)
        .single()
        .ok_or_else(|| anyhow!("failed to create synthetic base datetime"))?;

    Ok(base + synthetic_freq_step(freq, index as i64))
}

fn synthetic_freq_step(freq: Freq, index: i64) -> Duration {
    if let Some(minutes) = freq.minutes() {
        return Duration::minutes(minutes * index);
    }

    match freq {
        Freq::D => Duration::days(index),
        Freq::W => Duration::weeks(index),
        Freq::M => Duration::days(30 * index),
        Freq::S => Duration::days(90 * index),
        Freq::Y => Duration::days(365 * index),
        _ => Duration::minutes(index),
    }
}

fn build_channel(
    func_id: u32,
    data_len: usize,
    fractals: &[czsc_core::objects::fx::FX],
    finished_bis: &[czsc_core::objects::bi::BI],
    pivots: &[czsc_core::objects::zs::ZS],
    segs: &[Segment],
    buy_sell_points: &[BuySellPoint],
) -> Result<ChannelSeries> {
    let (kind, name) = resolve_channel_meta(func_id)?;
    let mut values = vec![0.0; data_len];

    match func_id {
        101 => fill_fractal_channel(&mut values, fractals, Mark::G),
        102 => fill_fractal_channel(&mut values, fractals, Mark::D),
        201 => fill_bi_points_channel(&mut values, finished_bis),
        202 => fill_bi_direction_channel(&mut values, finished_bis),
        203 => fill_seg_points_channel(&mut values, finished_bis, segs),
        204 => fill_seg_direction_channel(&mut values, finished_bis, segs),
        301 => fill_zs_channel(&mut values, pivots, ZsField::Upper),
        302 => fill_zs_channel(&mut values, pivots, ZsField::Lower),
        303 => fill_zs_channel(&mut values, pivots, ZsField::Middle),
        304 => fill_zs_boundary_channel(&mut values, pivots),
        401..=406 => fill_signal_channel(&mut values, buy_sell_points, func_id),
        901..=903 => {}
        _ => return Err(anyhow!("unsupported func_id: {func_id}")),
    }

    Ok(ChannelSeries {
        func_id,
        kind,
        name: name.to_string(),
        values,
    })
}

fn resolve_channel_meta(func_id: u32) -> Result<(i32, &'static str)> {
    let kind = match func_id {
        101 => (ChannelKind::ChannelFxTop as i32, "fx_top"),
        102 => (ChannelKind::ChannelFxBottom as i32, "fx_bottom"),
        201 => (ChannelKind::ChannelBiPoints as i32, "bi_points"),
        202 => (ChannelKind::ChannelBiDirection as i32, "bi_direction"),
        203 => (ChannelKind::ChannelSegPoints as i32, "seg_points"),
        204 => (ChannelKind::ChannelSegDirection as i32, "seg_direction"),
        301 => (ChannelKind::ChannelZsUpper as i32, "zs_upper"),
        302 => (ChannelKind::ChannelZsLower as i32, "zs_lower"),
        303 => (ChannelKind::ChannelZsMiddle as i32, "zs_middle"),
        304 => (ChannelKind::ChannelZsBoundary as i32, "zs_boundary"),
        401 => (ChannelKind::ChannelBuy1 as i32, "buy1"),
        402 => (ChannelKind::ChannelSell1 as i32, "sell1"),
        403 => (ChannelKind::ChannelBuy2 as i32, "buy2"),
        404 => (ChannelKind::ChannelSell2 as i32, "sell2"),
        405 => (ChannelKind::ChannelBuy3 as i32, "buy3"),
        406 => (ChannelKind::ChannelSell3 as i32, "sell3"),
        901 => (ChannelKind::ChannelDebugCacheHit as i32, "debug_cache_hit"),
        902 => (
            ChannelKind::ChannelDebugServiceReady as i32,
            "debug_service_ready",
        ),
        903 => (
            ChannelKind::ChannelDebugLatencyMs as i32,
            "debug_latency_ms",
        ),
        _ => return Err(anyhow!("unsupported func_id: {func_id}")),
    };

    Ok(kind)
}

fn fill_fractal_channel(values: &mut [f32], fractals: &[czsc_core::objects::fx::FX], mark: Mark) {
    for fx in fractals.iter().filter(|fx| fx.mark == mark) {
        if let Some(index) = fx_mid_index(fx) {
            values[index] = fx.fx as f32;
        }
    }
}

fn fill_bi_points_channel(values: &mut [f32], finished_bis: &[czsc_core::objects::bi::BI]) {
    for bi in finished_bis {
        // 每笔的起止分型都标注（此前只填终点 fx_b，第一笔起点缺失）
        if let Some(index) = fx_mid_index(&bi.fx_a) {
            values[index] = bi.fx_a.fx as f32;
        }
        if let Some(index) = fx_mid_index(&bi.fx_b) {
            values[index] = bi.fx_b.fx as f32;
        }
    }
}

fn fill_bi_direction_channel(values: &mut [f32], finished_bis: &[czsc_core::objects::bi::BI]) {
    for bi in finished_bis {
        let Some(start) = fx_mid_index(&bi.fx_a) else {
            continue;
        };
        let Some(end) = fx_mid_index(&bi.fx_b) else {
            continue;
        };

        let direction = match bi.direction {
            CzscDirection::Up => 1.0,
            CzscDirection::Down => -1.0,
        };

        for value in &mut values[start.min(end)..=start.max(end)] {
            *value = direction;
        }
    }
}

/// 段端点通道（203）：段首笔起点分型与末笔终点分型处输出分型价格。
///
/// **只输出已确认段**（`is_sure=true`）；尾部未完成/暂定段不显示（图表上只画
/// 特征序列分型确认的段）。段的起止都是笔端点（分型位置），方向由 101/102
/// 顶底分型通道判定：向上段起点为底分型、终点为顶分型；向下段反之。相邻段
/// 共享端点分型，重复写值幂等。
fn fill_seg_points_channel(values: &mut [f32], finished_bis: &[czsc_core::objects::bi::BI], segs: &[Segment]) {
    for seg in segs.iter().filter(|s| s.is_sure) {
        if let Some(start_bi) = finished_bis.get(seg.start_bi_idx)
            && let Some(index) = fx_mid_index(&start_bi.fx_a)
        {
            values[index] = start_bi.fx_a.fx as f32;
        }
        if let Some(end_bi) = finished_bis.get(seg.end_bi_idx)
            && let Some(index) = fx_mid_index(&end_bi.fx_b)
        {
            values[index] = end_bi.fx_b.fx as f32;
        }
    }
}

/// 段方向通道（204）：已确认段覆盖区间填 ±1（向上=1，向下=-1），
/// 与 202 笔向通道对称；未确认段不填。
fn fill_seg_direction_channel(values: &mut [f32], finished_bis: &[czsc_core::objects::bi::BI], segs: &[Segment]) {
    for seg in segs.iter().filter(|s| s.is_sure) {
        let Some(start_bi) = finished_bis.get(seg.start_bi_idx) else {
            continue;
        };
        let Some(end_bi) = finished_bis.get(seg.end_bi_idx) else {
            continue;
        };
        let Some(start) = fx_mid_index(&start_bi.fx_a) else {
            continue;
        };
        let Some(end) = fx_mid_index(&end_bi.fx_b) else {
            continue;
        };

        let direction = match seg.direction {
            CzscDirection::Up => 1.0,
            CzscDirection::Down => -1.0,
        };
        for value in &mut values[start.min(end)..=start.max(end)] {
            *value = direction;
        }
    }
}

enum ZsField {
    Upper,
    Lower,
    Middle,
}

/// 缠论规则构建中枢（rust-server 层）。
///
/// czsc-core 的 `get_zs_seq` 把"离开/连接笔"（第一根与前一中枢区间无交集的笔）
/// 并入下一个中枢作为其第一笔，导致相邻中枢共享端点（前一中枢 edt == 后一
/// 中枢 sdt），这不符合缠论"两个中枢之间至少间隔一笔"的要求。这里基于 czsc
/// 的已完成笔重新构建：
///   1. 中枢 = 至少 3 笔连续重叠，区间 [zd, zg] = 前 3 笔的重叠；
///   2. 后续与区间有交集的笔并入中枢（延伸），区间不变；缠论规定中枢延伸
///      不超过 9 段（9 笔），达到 9 笔后中枢结束——即使后续笔仍在区间内，
///      该情形属中枢升级，图上简化为从中枢结束后重新寻找新中枢；
///   3. 与区间无交集的笔为"离开/连接笔"，不属于任一中枢，作为两个中枢之间
///      的间隔；下一个中枢从连接笔之后开始寻找。
fn build_pivots(bis: &[BI]) -> Vec<ZS> {
    /// 缠论：中枢延伸最多 9 段（笔）。
    const MAX_ZS_BI_NUM: usize = 9;

    fn overlap_range(b: &[BI]) -> (f64, f64) {
        let zd = b.iter().map(|x| x.get_low()).fold(f64::NEG_INFINITY, f64::max);
        let zg = b.iter().map(|x| x.get_high()).fold(f64::INFINITY, f64::min);
        (zd, zg)
    }

    let mut pivots = Vec::new();
    let mut i = 0usize;
    while i + 3 <= bis.len() {
        // 寻找下一个"至少 3 笔重叠"的中枢起点（滑窗）
        let mut start = None;
        for j in i..=(bis.len() - 3) {
            let (zd, zg) = overlap_range(&bis[j..j + 3]);
            if zg >= zd {
                start = Some(j);
                break;
            }
        }
        let Some(j) = start else { break; };

        let (zd, zg) = overlap_range(&bis[j..j + 3]);
        // 延伸：后续与中枢区间有交集的笔并入，且中枢总笔数不超过 9 段
        let mut end = j + 3;
        while end < bis.len() && end - j < MAX_ZS_BI_NUM {
            let bi = &bis[end];
            if bi.get_high() >= zd && bi.get_low() <= zg {
                end += 1;
            } else {
                break;
            }
        }
        pivots.push(ZS::new(bis[j..end].to_vec()));
        // 下一中枢从 end+1 开始：
        // - 正常结束：bis[end] 是与区间无交集的离开笔（连接笔），跳过；
        // - 9 段截断：bis[end] 是第 10 笔（仍与区间重叠，属中枢升级），
        //   作为升级过渡跳过，避免与截断中枢共享端点。
        i = end + 1;
    }
    pivots
}

/// 中枢的绘制区间（通道对齐用）：排除进入段与离开段。
///
/// 缠论：中枢 = 至少 3 个连续次级别走势（笔）的重叠部分，中枢不包含进入段
/// （进入中枢前的走势）和离开段（离开中枢后的走势）。
///
/// 判断规则：
/// - **尾笔（离开段）**：其终点极值（fx_b.fx）**超出**中枢区间 [zd, zg]
///   （突破上沿/跌破下沿）才视为离开段予以排除；终点回到区间内（回抽回中枢）
///   属中枢延伸，应保留。
/// - **首笔（进入段）**：不"锚定"中枢区间（低点 != zd 且高点 != zg）的笔
///   才是进入段予以排除；锚定区间的笔是中枢定义笔，必须保留。
/// - 两类排除都保证剔除后核心仍不少于 3 笔，绘制的中枢始终包含至少 3 笔。
fn zs_core_span(zs: &ZS) -> Option<(usize, usize)> {
    let bis = &zs.bis;
    let len = bis.len();
    if len < 3 {
        return None;
    }
    // 离开段（尾笔）：终点极值超出中枢区间才排除；回中枢（终点在区间内）保留
    let last_end = bis[len - 1].fx_b.fx;
    let last_breaks_out = last_end < zs.zd || last_end > zs.zg;
    let end = if len >= 4 && last_breaks_out { len - 1 } else { len };
    // 进入段（首笔）：不锚定区间且剔除后剩余 ≥3 笔才排除
    let first_binds = bis[0].get_low() == zs.zd || bis[0].get_high() == zs.zg;
    let start = if end - 1 >= 3 && !first_binds { 1 } else { 0 };

    let start_index = bi_start_index(bis.get(start))?;
    let end_index = bi_end_index(bis.get(end - 1))?;
    Some((start_index.min(end_index), start_index.max(end_index)))
}

fn fill_zs_channel(values: &mut [f32], pivots: &[ZS], field: ZsField) {
    for zs in pivots {
        let Some((start, end)) = zs_core_span(zs) else {
            continue;
        };

        let pivot_value = match field {
            ZsField::Upper => zs.zg,
            ZsField::Lower => zs.zd,
            ZsField::Middle => zs.zz,
        } as f32;

        for value in &mut values[start..=end] {
            *value = pivot_value;
        }
    }
}

/// 中枢边界标记：每个中枢绘制区间首 K 线输出 1（左界）、尾 K 线输出 2（右界），
/// 其余为 0；既是某中枢右界又是另一中枢左界的共享 K 线输出 3（左右界共用）。
/// 配合 301/302 通道，通达信侧用折线/STICKLINE 画上/下沿与左右竖线，构成闭合
/// 矩形，且相邻中枢之间互不连接。
///
/// 相邻中枢共享端点K线（前一中枢的右界 == 后一中枢的左界），若按顺序直接
/// 写值，共享K线会被后一中枢的左界 1 覆盖，前一中枢的右界 2 丢失，图上该
/// 中枢只剩左侧竖线、没有上下边框和右边框。因此先统一写所有左界，再写右界；
/// 遇到已写过左界的共享K线则升级为 3，公式层把 3 同时当作左界和右界。
/// （`build_pivots` 已让相邻中枢间隔一笔，正常不会出现共享K线，这里保留
/// 3 的处理作为兜底。）
fn fill_zs_boundary_channel(values: &mut [f32], pivots: &[ZS]) {
    let mut starts = Vec::with_capacity(pivots.len());
    let mut ends = Vec::with_capacity(pivots.len());
    for zs in pivots {
        let Some((start, end)) = zs_core_span(zs) else {
            continue;
        };
        starts.push(start);
        ends.push(end);
    }

    for &start in &starts {
        if start < values.len() {
            values[start] = 1.0;
        }
    }
    for &end in &ends {
        if end < values.len() {
            values[end] = if values[end] > 0.5 { 3.0 } else { 2.0 };
        }
    }
}

/// 分型在原始K线序列中的索引（通道对齐用）。
///
/// czsc-core 处理包含关系时（`remove_include`），合并后的 NewBar 的 `id`
/// 锚定在合并组**第一根**原始K线，而其 `dt` 锚定在合并组的**极值**K线
/// （向下合并取最低低点所在K线、向上合并取最高高点所在K线）；分型的
/// `fx.dt` 即中间元素的 `dt`，也就是极值K线时间。若直接取 `elements[1].id`，
/// 分型会被画到合并组起点，造成分型漂移（例：002340 日线 2026-07 的底分型
/// 被画到 07-16 而非极值所在的 07-20）。因此这里在中间元素的原始K线里查找
/// `dt == fx.dt` 的那一根并取其 id；找不到时回退到 `elements[1].id`。
fn fx_mid_index(fx: &czsc_core::objects::fx::FX) -> Option<usize> {
    let mid = fx.elements.get(1)?;
    mid.elements
        .iter()
        .find(|bar| bar.dt == fx.dt)
        .map(|bar| bar.id as usize)
        .or(Some(mid.id as usize))
}

fn bi_start_index(bi: Option<&czsc_core::objects::bi::BI>) -> Option<usize> {
    bi.and_then(|bi| fx_mid_index(&bi.fx_a))
}

fn bi_end_index(bi: Option<&czsc_core::objects::bi::BI>) -> Option<usize> {
    bi.and_then(|bi| fx_mid_index(&bi.fx_b))
}

fn map_fractal_point(fx: &czsc_core::objects::fx::FX) -> Option<FractalPoint> {
    let index = fx_mid_index(fx)?;
    let mark = match fx.mark {
        Mark::G => FractalMark::FractalTop as i32,
        Mark::D => FractalMark::FractalBottom as i32,
    };

    Some(FractalPoint {
        index: index as u32,
        mark,
        price: fx.fx,
        high: fx.high,
        low: fx.low,
    })
}

fn map_stroke_segment(bi: &czsc_core::objects::bi::BI) -> Option<StrokeSegment> {
    let start_index = fx_mid_index(&bi.fx_a)?;
    let end_index = fx_mid_index(&bi.fx_b)?;

    Some(StrokeSegment {
        start_index: start_index as u32,
        end_index: end_index as u32,
        direction: match bi.direction {
            CzscDirection::Up => crate::proto::analysis::Direction::Up as i32,
            CzscDirection::Down => crate::proto::analysis::Direction::Down as i32,
        },
        start_price: bi.fx_a.fx,
        end_price: bi.fx_b.fx,
        high: bi.get_high(),
        low: bi.get_low(),
    })
}

fn map_pivot_zone(zs: &ZS) -> Option<PivotZone> {
    // 绘制区间排除进入段/离开段（首/尾笔），与通道保持一致
    let (start_index, end_index) = zs_core_span(zs)?;

    Some(PivotZone {
        start_index: start_index as u32,
        end_index: end_index as u32,
        upper: zs.zg,
        lower: zs.zd,
        middle: zs.zz,
    })
}

fn map_segment(seg: &Segment, bis: &[czsc_core::objects::bi::BI]) -> Option<ProtoSegment> {
    let start_bi = bis.get(seg.start_bi_idx)?;
    let end_bi = bis.get(seg.end_bi_idx)?;
    let start_index = fx_mid_index(&start_bi.fx_a)?;
    let end_index = fx_mid_index(&end_bi.fx_b)?;

    Some(ProtoSegment {
        start_bi_index: seg.start_bi_idx as u32,
        end_bi_index: seg.end_bi_idx as u32,
        start_index: start_index as u32,
        end_index: end_index as u32,
        direction: match seg.direction {
            CzscDirection::Up => crate::proto::analysis::Direction::Up as i32,
            CzscDirection::Down => crate::proto::analysis::Direction::Down as i32,
        },
        start_price: start_bi.fx_a.fx,
        end_price: end_bi.fx_b.fx,
        high: start_bi.get_high().max(end_bi.get_high()),
        low: start_bi.get_low().min(end_bi.get_low()),
        is_sure: seg.is_sure,
    })
}

fn fill_signal_channel(values: &mut [f32], buy_sell_points: &[BuySellPoint], func_id: u32) {
    for point in buy_sell_points.iter().filter(|p| p.func_id == func_id) {
        values[point.index] = point.price as f32;
    }
}

/// 三买/三卖缠论严格化：**依附中枢，每个中枢最多一个三买（405）和一个三卖（406）**。
///
/// 三买/三卖按"锚点之前最近结束的中枢"归属（三买/三卖是中枢被突破/跌破后的
/// 回抽确认点，出现在该中枢结束之后）；同一中枢的第二个同向点丢弃；锚点前
/// 无任何已结束中枢的点（无法归属）也丢弃。
fn strictize_third_bs_by_zs(points: &mut Vec<BuySellPoint>, pivots: &[ZS]) {
    let mut seen_405: Vec<usize> = Vec::new();
    let mut seen_406: Vec<usize> = Vec::new();
    points.retain(|p| {
        if p.func_id != 405 && p.func_id != 406 {
            return true;
        }
        // 归属到锚点之前最近结束的中枢（用中枢组最后一笔终点）
        let Some(zi) = pivots.iter().rposition(|zs| {
            bi_end_index(zs.bis.last()).map_or(false, |e| e < p.index)
        }) else {
            return false;
        };
        if p.func_id == 405 {
            if seen_405.contains(&zi) {
                return false;
            }
            seen_405.push(zi);
        } else {
            if seen_406.contains(&zi) {
                return false;
            }
            seen_406.push(zi);
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::proto::analysis::{AnalyzeRequest, CzscOptions, MarketSeries};
    use crate::state::AppState;

    fn sample_close_series() -> Vec<f32> {
        vec![
            10.0, 10.2, 10.5, 10.1, 9.8, 9.6, 9.9, 10.3, 10.7, 10.4, 10.1, 9.7, 9.5, 9.9, 10.4,
            10.9, 10.6, 10.1, 9.8, 9.4, 9.7, 10.2, 10.8, 11.2, 10.9, 10.5, 10.0, 9.6, 9.9, 10.3,
            10.7, 11.0, 10.8, 10.3, 9.9, 9.5, 9.8, 10.1, 10.6, 11.1, 10.7, 10.2, 9.7, 9.3, 9.6,
            10.0, 10.5, 10.9, 10.6, 10.1, 9.8, 9.4, 9.7, 10.2, 10.6, 11.0, 10.8, 10.4, 10.0, 9.7,
            10.0, 10.4, 10.9, 11.3,
        ]
    }

    fn sample_request(include_objects: bool) -> AnalyzeRequest {
        let close = sample_close_series();
        let mut high = Vec::with_capacity(close.len());
        let mut low = Vec::with_capacity(close.len());
        for (i, price) in close.iter().enumerate() {
            high.push(*price + 0.25 + (i % 3) as f32 * 0.03);
            low.push(*price - 0.22 - (i % 2) as f32 * 0.02);
        }

        AnalyzeRequest {
            func_group: "single_level_structure".to_string(),
            requested_func_ids: vec![101, 102, 201, 202, 301, 302, 303],
            symbol_hint: "000001.SZ".to_string(),
            freq_hint: "F30".to_string(),
            data_len: close.len() as u32,
            input_hash: 0xAABBCCDD,
            bar_build_mode: crate::proto::analysis::BarBuildMode::BarBuildFromHlc3 as i32,
            series: Some(MarketSeries {
                open: Vec::new(),
                high,
                low,
                close,
                vol: Vec::new(),
                amount: Vec::new(),
                dt_unix_ms: Vec::new(),
            }),
            options: Some(CzscOptions {
                max_bi_num: 50,
                min_bi_len: 3,
                analysis_window: 64,
                include_channels: true,
                include_objects,
                enable_incremental: false,
            }),
            prefer_cached: false,
            caller_instance_id: 7,
        }
    }

    fn test_state() -> std::sync::Arc<AppState> {
        let mut config = AppConfig::default();
        config.min_bi_len = 3;
        config.default_analysis_window = 64;
        AppState::new(config)
    }

    fn channel(snapshot: &AnalysisSnapshot, func_id: u32) -> &ChannelSeries {
        snapshot
            .channels
            .iter()
            .find(|x| x.func_id == func_id)
            .unwrap_or_else(|| panic!("channel {func_id} not found"))
    }

    fn nonzero_count(values: &[f32]) -> usize {
        values.iter().filter(|x| x.abs() > f32::EPSILON).count()
    }

    #[test]
    fn build_snapshot_populates_core_channels_and_objects() {
        let state = test_state();
        let request = sample_request(true);
        let snapshot = build_snapshot(&request, state.as_ref()).expect("snapshot should build");

        assert_eq!(snapshot.meta.as_ref().unwrap().data_len, request.data_len);
        assert_eq!(snapshot.channels.len(), 7);

        assert!(
            nonzero_count(&channel(&snapshot, 101).values) > 0,
            "fx_top should have non-zero values"
        );
        assert!(
            nonzero_count(&channel(&snapshot, 102).values) > 0,
            "fx_bottom should have non-zero values"
        );
        assert!(
            nonzero_count(&channel(&snapshot, 201).values) > 0,
            "bi_points should have non-zero values"
        );
        assert!(
            nonzero_count(&channel(&snapshot, 202).values) > 0,
            "bi_direction should have non-zero values"
        );
        assert!(
            nonzero_count(&channel(&snapshot, 301).values) > 0,
            "zs_upper should have non-zero values"
        );
        assert!(
            nonzero_count(&channel(&snapshot, 302).values) > 0,
            "zs_lower should have non-zero values"
        );
        assert!(
            nonzero_count(&channel(&snapshot, 303).values) > 0,
            "zs_middle should have non-zero values"
        );

        assert!(
            !snapshot.fractals.is_empty(),
            "fractals should be returned when include_objects=true"
        );
        assert!(
            !snapshot.strokes.is_empty(),
            "strokes should be returned when include_objects=true"
        );
        assert!(
            !snapshot.pivots.is_empty(),
            "pivots should be returned when include_objects=true"
        );
    }

    #[test]
    fn compute_buy_sell_points_runs_on_sample_data() {
        let state = test_state();
        let request = sample_request(false);
        let bars = build_raw_bars(&request, Freq::F30, 0).expect("bars should build");
        let points = compute_buy_sell_points(&bars, 50, 3);
        // 样例数据（64 根、15 笔）可命中 403 二买；信号产出取决于数据形态，
        // 这里只验证与 czsc-signals 的接入管线可运行。
        println!("buy/sell points on sample data: {points:?}");
        assert!(points.iter().all(|p| (401..=406).contains(&p.func_id)));
    }

    #[test]
    fn dump_channel_detail_consistency() {
        let state = test_state();
        let mut request = sample_request(false);
        request.requested_func_ids = vec![101, 102, 201, 202, 301, 302, 303];
        let snapshot = build_snapshot(&request, state.as_ref()).expect("snapshot");

        let ch = |id: u32| snapshot.channels.iter().find(|c| c.func_id == id).unwrap();
        let fx_top = ch(101);
        let fx_bot = ch(102);
        let bi = ch(201);
        let bid = ch(202);
        let zsu = ch(301);
        let zsl = ch(302);

        println!("== 笔端点与分型/笔向对照 ==");
        for (i, v) in bi.values.iter().enumerate() {
            if v.abs() > f32::EPSILON {
                println!(
                    "BI idx={i:<3} price={v:<7} fx_top={:<7} fx_bot={:<7} bid={}",
                    fx_top.values[i], fx_bot.values[i], bid.values[i]
                );
            }
        }

        println!("\n== 中枢上下沿区间 ==");
        let nz: Vec<usize> = zsu
            .values
            .iter()
            .enumerate()
            .filter(|(_, v)| v.abs() > 1e-6)
            .map(|(i, _)| i)
            .collect();
        if let (Some(first), Some(last)) = (nz.first(), nz.last()) {
            println!("ZS idx range: {first}..{last} count={}", nz.len());
            let holes: Vec<usize> = nz
                .windows(2)
                .filter(|w| w[1] - w[0] > 1)
                .map(|w| w[0])
                .collect();
            println!("ZS holes(不连续): {holes:?}");
            println!("ZS 每根bar值: {:?}", &zsu.values[*first..=*last]);
            println!("ZL 每根bar值: {:?}", &zsl.values[*first..=*last]);
        } else {
            println!("ZS 无区间");
        }
    }

    #[test]
    fn profile_analysis_stages() {
        use std::time::Instant;
        let state = test_state();
        let request = sample_request(false);
        let series = request.series.as_ref().unwrap();

        let t0 = Instant::now();
        let bars = build_raw_bars(&request, Freq::F30, 0).expect("bars");
        let t1 = Instant::now();
        let czsc = CZSC::new(bars.clone(), 50, 3);
        let t2 = Instant::now();
        let points = compute_buy_sell_points(&bars, 50, 3);
        let t3 = Instant::now();
        let pivots = build_pivots(&czsc.get_finished_bis());
        let t4 = Instant::now();

        println!(
            "bars={} build_raw_bars={:?} czsc_new={:?} signals={:?} zs={:?} points={}",
            bars.len(),
            t1 - t0,
            t2 - t1,
            t3 - t2,
            t4 - t3,
            points.len()
        );
        let _ = series;
    }

    #[test]
    fn validate_series_lengths_rejects_mismatch() {
        let bad_series = crate::proto::analysis::MarketSeries {
            open: Vec::new(),
            high: vec![1.0, 2.0],
            low: vec![1.0],
            close: vec![1.0, 2.0],
            vol: Vec::new(),
            amount: Vec::new(),
            dt_unix_ms: Vec::new(),
        };

        let err =
            validate_series_lengths(2, &bad_series).expect_err("mismatched low series should fail");
        assert!(err.to_string().contains("series low length"));
    }

    #[test]
    fn build_snapshot_with_signal_channels_does_not_panic() {
        let state = test_state();
        let mut request = sample_request(false);
        request.requested_func_ids = vec![401, 402, 403, 404, 405, 406];
        let snapshot = build_snapshot(&request, state.as_ref()).expect("snapshot should build");

        assert_eq!(snapshot.channels.len(), 6);
        for func_id in [401, 402, 403, 404, 405, 406] {
            assert_eq!(channel(&snapshot, func_id).values.len(), 64);
        }
    }

    #[test]
    fn parse_freq_accepts_short_aliases() {
        assert_eq!(parse_freq("30m").unwrap(), Freq::F30);
        assert_eq!(parse_freq("D").unwrap(), Freq::D);
        assert_eq!(parse_freq("1d").unwrap(), Freq::D);
    }

    /// 回归测试：分型漂移（对应 002340 日线 2026-07 底分型被画到 07-16 而非 07-20）。
    ///
    /// czsc-core 在去除包含关系时，合并后的 NewBar 的 `id` 锚定在合并组**第一根**
    /// 原始K线，而 `dt` 锚定在合并组**极值**K线（向下合并取最低低点所在K线）。
    /// 分型定位若直接用 `fx.elements[1].id`，会把分型画到合并组起点而非极值K线。
    ///
    /// 这里构造一段下行包含链：idx3..idx5 合并为一根 NewBar（id=3，dt=idx5 的
    /// 时间，最低低点 12.0 在 idx5），其后 idx6 向上突破。底分型（fx=12.0）的
    /// 极值位置应为 idx5，而不是合并组起点的 idx3。
    #[test]
    fn merged_bar_fractal_placed_at_extreme_bar_not_merge_group_start() {
        let highs = [12.0f32, 13.0, 13.5, 13.2, 13.3, 13.2, 13.4, 13.8];
        let lows = [11.0f32, 11.5, 12.8, 12.4, 12.2, 12.0, 12.3, 12.9];

        let request = AnalyzeRequest {
            func_group: "single_level_structure".to_string(),
            requested_func_ids: vec![101, 102],
            symbol_hint: "TEST".to_string(),
            freq_hint: "D".to_string(),
            data_len: highs.len() as u32,
            input_hash: 0,
            bar_build_mode: crate::proto::analysis::BarBuildMode::BarBuildFromHlc3 as i32,
            series: Some(MarketSeries {
                open: Vec::new(),
                high: highs.to_vec(),
                low: lows.to_vec(),
                close: highs.to_vec(),
                vol: Vec::new(),
                amount: Vec::new(),
                dt_unix_ms: Vec::new(),
            }),
            options: Some(crate::proto::analysis::CzscOptions {
                max_bi_num: 50,
                min_bi_len: 6,
                analysis_window: 0, // 0 -> 使用 state 默认值 64，窗口覆盖全部 8 根
                include_channels: true,
                include_objects: false,
                enable_incremental: false,
            }),
            prefer_cached: false,
            caller_instance_id: 0,
        };

        let state = test_state();
        let snapshot = build_snapshot(&request, state.as_ref()).expect("snapshot should build");

        let fx_bottom = channel(&snapshot, 102);
        // 底分型必须落在极值K线（idx5，low=12.0），而不是合并组起点 idx3。
        assert!(
            (fx_bottom.values[5] - 12.0).abs() < 1e-4,
            "bottom fractal should be placed at idx5 (extreme bar), got {}",
            fx_bottom.values[5]
        );
        assert_eq!(fx_bottom.values[3], 0.0, "no fractal at merge-group start idx3");

        let fx_top = channel(&snapshot, 101);
        assert!(
            (fx_top.values[2] - 13.5).abs() < 1e-4,
            "top fractal should stay at idx2, got {}",
            fx_top.values[2]
        );
    }
}
