//! 分析快照编排（rust-server 层）。
//!
//! 职责：请求校验、组装分析管线（RawBar → CZSC 分型/笔 → 中枢/段/买卖点 →
//! 通道与对象映射 → `AnalysisSnapshot`）、参数解析（请求选项回退到配置）。
//!
//! 模块划分（每个模块只承担一个职责）：
//! - [`rawbars`]：请求 → RawBar 转换（频率解析、量能来源链、时间轴合成）；
//! - [`pivots`]：中枢构建（原生 `get_zs_seq` + 前端/后端优化）与三买/三卖严格化；
//! - [`segs`]：线段识别（特征序列分型法）；
//! - [`signals`]：401~406 买卖点信号；
//! - [`channels`]：func_id → 对齐 DataLen 的 float[] 输出通道；
//! - [`objects`]：分型/笔/中枢/段 → proto 对象映射与 K 线下标对齐。

mod channels;
mod error;
mod incremental;
mod objects;
mod pivots;
mod rawbars;
mod segs;
mod signals;

#[cfg(test)]
mod fixtures;

// 对外 API 保持原路径（`analyzer::xxx`），内部实现已按模块拆分。
pub(crate) use channels::SUPPORTED_FUNC_IDS;
pub use channels::build_channel;
pub use error::AnalysisError;
pub(crate) use incremental::{AcceptOutcome, IncrementalCache, IncrementalKey, IncrementalState};
pub use pivots::build_pivots;
pub use rawbars::{build_raw_bars, parse_freq};
pub use segs::{Segment, build_segs};
pub(crate) use signals::evolve_bars;
pub use signals::{BuySellPoint, compute_buy_sell_points};

use czsc_core::analyze::CZSC;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::AppConfig;
use crate::proto::analysis::{
    AnalysisSnapshot, AnalyzeRequest, FractalPoint, PivotZone, SignalPoint, SnapshotMeta,
    StrokeSegment,
};

use objects::map_segment;
use pivots::strictize_third_bs_by_zs;

/// 分析参数：`build_snapshot` 唯一的运行时输入。
///
/// 由请求选项 + 服务配置解析而来（选项优先、配置兜底），使分析管线成为
/// 不依赖运行时状态的纯函数——单元测试与未来多周期/增量分析可直接构造。
#[derive(Debug, Clone)]
pub struct AnalysisParams {
    /// 分析窗口（bar 数）
    pub analysis_window: u32,
    pub max_bi_num: u32,
    pub min_bi_len: u32,
    /// vol 缺失时是否派生振幅代理量（见 rawbars 量能来源链）
    pub derive_volume_proxy: bool,
    /// 分析器版本标识（写入快照 meta）
    pub czsc_version: String,
}

impl From<&AppConfig> for AnalysisParams {
    fn from(config: &AppConfig) -> Self {
        Self {
            analysis_window: config.default_analysis_window,
            max_bi_num: config.max_bi_num,
            min_bi_len: config.min_bi_len,
            derive_volume_proxy: config.derive_volume_proxy,
            czsc_version: config.czsc_version.clone(),
        }
    }
}

impl From<(&AnalyzeRequest, &AppConfig)> for AnalysisParams {
    fn from((request, config): (&AnalyzeRequest, &AppConfig)) -> Self {
        let options = request.options.as_ref();
        Self {
            analysis_window: options
                .map(|x| x.analysis_window)
                .filter(|x| *x > 0)
                .unwrap_or(config.default_analysis_window),
            max_bi_num: options
                .map(|x| x.max_bi_num)
                .filter(|x| *x > 0)
                .unwrap_or(config.max_bi_num),
            min_bi_len: options
                .map(|x| x.min_bi_len)
                .filter(|x| *x > 0)
                .unwrap_or(config.min_bi_len),
            derive_volume_proxy: config.derive_volume_proxy,
            czsc_version: config.czsc_version.clone(),
        }
    }
}

/// 执行一次全量分析并生成快照（CPU 密集，由调用方放入阻塞线程池）。
///
/// 全量路径：请求校验 → 窗口切片 → 单遍演化（`evolve_bars`，结构+信号
/// 共用一次逐根演化）→ `assemble_snapshot`。增量路径（`enable_incremental`）
/// 由 handlers 层先行尝试 `incremental` 模块的承接，未命中时回退本函数。
pub fn build_snapshot(
    request: &AnalyzeRequest,
    params: &AnalysisParams,
) -> Result<AnalysisSnapshot, AnalysisError> {
    let options = request.options.as_ref();
    let data_len = request.data_len as usize;
    let series = request
        .series
        .as_ref()
        .ok_or_else(|| AnalysisError::invalid_request("market series is missing"))?;

    validate_series_lengths(request.data_len, series)?;

    let analysis_window = options
        .map(|x| x.analysis_window)
        .filter(|x| *x > 0)
        .unwrap_or(params.analysis_window);

    let freq = parse_freq(&request.freq_hint)?;
    let window_start = window_start_of(data_len, analysis_window);
    let bars = build_raw_bars(request, freq, window_start, params.derive_volume_proxy)?;

    if bars.is_empty() {
        return Err(AnalysisError::invalid_request(
            "bars are empty after applying analysis window",
        ));
    }

    let max_bi_num = options
        .map(|x| x.max_bi_num)
        .filter(|x| *x > 0)
        .unwrap_or(params.max_bi_num) as usize;
    let min_bi_len = options
        .map(|x| x.min_bi_len)
        .filter(|x| *x > 0)
        .unwrap_or(params.min_bi_len) as usize;

    let wants_signals = requested_func_ids(request)
        .iter()
        .any(|id| (401..=406).contains(id));
    // 单遍演化（P0）：结构+信号共用一次逐根演化（见 signals::evolve_bars），
    // 避免"信号演化 + 结构 CZSC::new"的双重全量扫描。
    let (czsc, evaluator) = evolve_bars(&bars, max_bi_num, min_bi_len, wants_signals);
    let buy_sell_points = match evaluator {
        Some(mut ev) => ev.finalize_points(&czsc),
        None => Vec::new(),
    };

    assemble_snapshot(
        request,
        params,
        freq,
        max_bi_num,
        min_bi_len,
        &czsc,
        buy_sell_points,
    )
}

/// 请求的通道号列表（空时使用默认结构通道集）。
fn requested_func_ids(request: &AnalyzeRequest) -> Vec<u32> {
    if request.requested_func_ids.is_empty() {
        vec![101, 102, 201, 301, 302]
    } else {
        request.requested_func_ids.clone()
    }
}

/// 分析窗口起点（全量/限窗的统一表达）：
/// `analysis_window == 0` 表示**全量**（从第 1 根 K 线起，前方历史
/// 全部参与分析，配合增量缓存稳态维护）；否则取尾部 `analysis_window`
/// 根（`data_len - analysis_window`，负则取 0）。
///
/// 全量演进说明：固定尾窗口下序列增长会使窗口整体平移，增量分析
/// （`incremental.rs`）无法承接而回退全量；`0 = 全量` 时窗口首部稳定，
/// 增量承接（序列增长/盘中原地更新）持续生效。
pub fn window_start_of(data_len: usize, analysis_window: u32) -> usize {
    let window = analysis_window as usize;
    if window == 0 {
        0
    } else {
        data_len.saturating_sub(window)
    }
}

/// 由已演化的 CZSC 与确认的买卖点装配快照（全量路径与增量路径共用）
///
/// 内部完成：分型/笔列表提取、中枢（前端/后端优化）、段（特征序列
/// 分型法）、三买/三卖严格化、通道构建、对象映射与快照 meta。
pub(crate) fn assemble_snapshot(
    request: &AnalyzeRequest,
    params: &AnalysisParams,
    freq: czsc_core::objects::freq::Freq,
    max_bi_num: usize,
    min_bi_len: usize,
    czsc: &CZSC,
    mut buy_sell_points: Vec<BuySellPoint>,
) -> Result<AnalysisSnapshot, AnalysisError> {
    let options = request.options.as_ref();
    let data_len = request.data_len as usize;

    let fractals = czsc.get_fx_list();
    // 已完成的笔（get_finished_bis 已处理"最后一笔待确认"的实时场景：
    // 无包含K线不足 5 根时自动剔除最后一笔）。
    let finished_bis = czsc.get_finished_bis();
    // 中枢：原生 czsc get_zs_seq 输出，再做前端/后端优化（见 pivots.rs）。
    let pivots = build_pivots(&finished_bis);
    // 缠论规则构建段（见 segs.rs：特征序列分型法，段 = 至少 3 笔、首尾同向）。
    let segs = build_segs(&finished_bis);
    // 三买/三卖依附中枢严格化：每个中枢最多一个三买或三卖。
    strictize_third_bs_by_zs(&mut buy_sell_points, &pivots);

    let channels = requested_func_ids(request)
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
        .collect::<Result<Vec<_>, AnalysisError>>()?;

    let generated_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| AnalysisError::invalid_request(e.to_string()))?
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
            analysis_window: params.analysis_window,
            generated_at_unix_ms,
            analyzer_version: params.czsc_version.clone(),
        }),
        channels,
        fractals: if include_objects {
            fractals
                .iter()
                .filter_map(|fx| FractalPoint::try_from(fx).ok())
                .collect()
        } else {
            Vec::new()
        },
        strokes: if include_objects {
            finished_bis
                .iter()
                .filter_map(|bi| StrokeSegment::try_from(bi).ok())
                .collect()
        } else {
            Vec::new()
        },
        pivots: if include_objects {
            pivots
                .iter()
                .filter_map(|zs| PivotZone::try_from(zs).ok())
                .collect()
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
            segs.iter()
                .filter_map(|s| map_segment(s, &finished_bis))
                .collect()
        } else {
            Vec::new()
        },
    })
}

/// 校验行情序列与 `data_len` 对齐（必需序列非空、可选序列长度一致）。
pub(crate) fn validate_series_lengths(
    data_len: u32,
    series: &crate::proto::analysis::MarketSeries,
) -> Result<(), AnalysisError> {
    let expected = data_len as usize;

    let check = |name: &str, values: &[f32], required: bool| -> Result<(), AnalysisError> {
        if values.is_empty() {
            if required {
                return Err(AnalysisError::invalid_request(format!(
                    "required series {name} is empty"
                )));
            }
            return Ok(());
        }
        if values.len() != expected {
            return Err(AnalysisError::invalid_request(format!(
                "series {name} length {} != data_len {expected}",
                values.len()
            )));
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
        return Err(AnalysisError::invalid_request(format!(
            "series dt_unix_ms length {} != data_len {expected}",
            series.dt_unix_ms.len()
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::fixtures::{channel, nonzero_count, sample_request, test_params};
    use crate::proto::analysis::{AnalyzeRequest, CzscOptions, MarketSeries};

    #[test]
    fn build_snapshot_populates_core_channels_and_objects() {
        let request = sample_request(true);
        let params = test_params(&request);
        let snapshot = build_snapshot(&request, &params).expect("snapshot should build");

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
        let request = sample_request(false);
        let bars = build_raw_bars(&request, czsc_core::objects::freq::Freq::F30, 0, true)
            .expect("bars should build");
        let (czsc, points) = compute_buy_sell_points(&bars, 50, 3);
        assert!(!czsc.bi_list.is_empty(), "演化终态的笔序列非空");
        // 样例数据（64 根、15 笔）可命中 403 二买；信号产出取决于数据形态，
        // 这里只验证与 czsc-signals 的接入管线可运行。
        println!("buy/sell points on sample data: {points:?}");
        assert!(points.iter().all(|p| (401..=406).contains(&p.func_id)));
    }

    #[test]
    fn build_snapshot_with_signal_channels_does_not_panic() {
        let mut request = sample_request(false);
        request.requested_func_ids = vec![401, 402, 403, 404, 405, 406];
        let params = test_params(&request);
        let snapshot = build_snapshot(&request, &params).expect("snapshot should build");

        assert_eq!(snapshot.channels.len(), 6);
        for func_id in [401, 402, 403, 404, 405, 406] {
            assert_eq!(channel(&snapshot, func_id).values.len(), 64);
        }
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
            options: Some(CzscOptions {
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

        let params = test_params(&request);
        let snapshot = build_snapshot(&request, &params).expect("snapshot should build");

        let fx_bottom = channel(&snapshot, 102);
        // 底分型必须落在极值K线（idx5，low=12.0），而不是合并组起点 idx3。
        assert!(
            (fx_bottom.values[5] - 12.0).abs() < 1e-4,
            "bottom fractal should be placed at idx5 (extreme bar), got {}",
            fx_bottom.values[5]
        );
        assert_eq!(
            fx_bottom.values[3], 0.0,
            "no fractal at merge-group start idx3"
        );

        let fx_top = channel(&snapshot, 101);
        assert!(
            (fx_top.values[2] - 13.5).abs() < 1e-4,
            "top fractal should stay at idx2, got {}",
            fx_top.values[2]
        );
    }

    /// 调试例程：分型/笔端点/笔向对照打印（配合 --nocapture 人工核对）。
    #[test]
    fn dump_channel_detail_consistency() {
        let mut request = sample_request(false);
        request.requested_func_ids = vec![101, 102, 201, 202, 301, 302, 303];
        let params = test_params(&request);
        let snapshot = build_snapshot(&request, &params).expect("snapshot");

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
            // 关键断言：中枢区间必须连续且与上下沿同时存在
            assert!(holes.is_empty(), "中枢填充不得出现空洞: {holes:?}");
            assert!(
                zsl.values[*first..=*last]
                    .iter()
                    .any(|v| v.abs() > f32::EPSILON),
                "中枢下沿应与上沿同时覆盖"
            );
        } else {
            println!("ZS 无区间");
        }
    }

    /// 性能剖面例程：打印各阶段耗时（配合 --nocapture 观察）。
    /// P0 后信号演化即结构计算（单遍），不再有独立的 czsc_new 阶段。
    #[test]
    fn profile_analysis_stages() {
        use std::time::Instant;
        let request = sample_request(false);

        let t0 = Instant::now();
        let bars =
            build_raw_bars(&request, czsc_core::objects::freq::Freq::F30, 0, true).expect("bars");
        let t1 = Instant::now();
        let (czsc, points) = compute_buy_sell_points(&bars, 50, 3);
        let t2 = Instant::now();

        println!(
            "bars={} build_raw_bars={:?} czsc+signals_single_pass={:?} bis={} points={}",
            bars.len(),
            t1 - t0,
            t2 - t1,
            czsc.bi_list.len(),
            points.len()
        );
    }

    /// 全量演进：`analysis_window == 0` 表示全量（窗口起点为 0），
    /// 正数取尾部 N 根（不足 N 时起点为 0）。
    #[test]
    fn window_start_zero_means_full_series() {
        assert_eq!(window_start_of(3933, 0), 0, "0 = 全量");
        assert_eq!(window_start_of(3933, 1000), 2933, "尾部 1000 根");
        assert_eq!(window_start_of(100, 1000), 0, "不足窗口时取全部");
        assert_eq!(window_start_of(0, 0), 0);
    }

    /// 0 = 全量：快照通道覆盖全部传入 K 线，meta 记录 analysis_window=0。
    #[test]
    fn analysis_window_zero_snapshot_covers_all_bars() {
        let mut request = sample_request(true);
        request.requested_func_ids = vec![101, 201, 301];
        request.options.as_mut().unwrap().analysis_window = 0;
        // 新默认：AppConfig::default() 的 default_analysis_window = 0（全量）
        let params = AnalysisParams::from((&request, &AppConfig::default()));
        assert_eq!(params.analysis_window, 0);

        let snapshot = build_snapshot(&request, &params).expect("snapshot");
        let data_len = request.data_len as usize;
        assert_eq!(snapshot.meta.as_ref().unwrap().data_len as usize, data_len);
        assert_eq!(
            snapshot.meta.as_ref().unwrap().analysis_window,
            0,
            "meta 应记录 0 = 全量"
        );
        for ch in &snapshot.channels {
            assert_eq!(
                ch.values.len(),
                data_len,
                "通道 {} 必须覆盖全部 K 线",
                ch.func_id
            );
        }
    }
}
