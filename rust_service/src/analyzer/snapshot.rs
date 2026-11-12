//! 快照装配：已演化的 CZSC + 买卖点 → `AnalysisSnapshot`。
//!
//! 装配阶段只消费 [`AnalysisSettings`]，避免把请求、参数、频率、窗口和
//! 通道列表重复展开成多个参数。

use std::time::{SystemTime, UNIX_EPOCH};

use czsc_core::analyze::CZSC;

use crate::proto::analysis::{
    AnalysisSnapshot, FractalPoint, PivotZone, SignalPoint, SnapshotMeta, StrokeSegment,
};

use super::channels::ChannelBuilder;
use super::error::AnalysisError;
use super::pivots::{PivotAnalyzer, SegmentPivotAnalyzer};
use super::segs::SegmentAnalyzer;
use super::settings::AnalysisSettings;
use super::signals::BuySellPoint;

/// 分析阶段耗时剖面（可选收集；不传则零开销）。
#[derive(Debug, Default, Clone)]
pub struct SnapshotProfile {
    /// 窗口切片 → RawBar（RawBarFactory）
    pub rawbars: std::time::Duration,
    /// 单遍演化：结构+信号共用一次逐 bar 演化（EvolutionEngine）
    pub evolve: std::time::Duration,
    /// evolve 内部：逐根 `czsc.update_bar`
    pub evolve_update: std::time::Duration,
    /// evolve 内部：信号评估
    pub evolve_observe: std::time::Duration,
    /// 中枢 + 段 + 三买/三卖严格化
    pub pivots_segs: std::time::Duration,
    /// 通道填充
    pub channels: std::time::Duration,
    /// 对象映射
    pub objects: std::time::Duration,
}

/// 由已演化的 CZSC 与确认买卖点装配快照。
pub(crate) struct SnapshotAssembler<'s, 'a> {
    settings: &'s AnalysisSettings<'a>,
    czsc: &'s CZSC,
    buy_sell_points: Vec<BuySellPoint>,
}

impl<'s, 'a> SnapshotAssembler<'s, 'a> {
    pub(crate) fn new(
        settings: &'s AnalysisSettings<'a>,
        czsc: &'s CZSC,
        buy_sell_points: Vec<BuySellPoint>,
    ) -> Self {
        Self {
            settings,
            czsc,
            buy_sell_points,
        }
    }

    pub(crate) fn assemble(
        mut self,
        mut profile: Option<&mut SnapshotProfile>,
    ) -> Result<AnalysisSnapshot, AnalysisError> {
        let data_len = self.settings.request().data_len as usize;
        let fractals = self.czsc.get_fx_list();
        let finished_bis = self.czsc.get_finished_bis();

        let t0 = std::time::Instant::now();
        let pivots = PivotAnalyzer::new(&finished_bis).build();
        let segs = SegmentAnalyzer::new(&finished_bis).build();
        let segment_pivots = SegmentPivotAnalyzer::new(&finished_bis, &segs).build();
        PivotAnalyzer::strictize_third_points(&mut self.buy_sell_points, &pivots);
        if let Some(p) = profile.as_deref_mut() {
            p.pivots_segs += t0.elapsed();
        }

        let t1 = std::time::Instant::now();
        let channels = ChannelBuilder::new(data_len)
            .fractals(&fractals)
            .bis(&finished_bis)
            .pivots(&pivots)
            .segment_pivots(&segment_pivots)
            .segs(&segs)
            .buy_sell_points(&self.buy_sell_points)
            .build_all(self.settings.requested_func_ids())?;
        if let Some(p) = profile.as_deref_mut() {
            p.channels += t1.elapsed();
        }

        let generated_at_unix_ms = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| AnalysisError::internal(e.to_string()))?
                .as_millis(),
        )
        .unwrap_or(u64::MAX);

        let t2 = std::time::Instant::now();
        let (fractals_obj, strokes_obj, pivots_obj, signals_obj, segments_obj, segment_pivots_obj) =
            if self.settings.include_objects() {
                (
                    fractals
                        .iter()
                        .filter_map(|fx| FractalPoint::try_from(fx).ok())
                        .collect::<Vec<_>>(),
                    finished_bis
                        .iter()
                        .filter_map(|bi| StrokeSegment::try_from(bi).ok())
                        .collect::<Vec<_>>(),
                    pivots
                        .iter()
                        .filter_map(|zs| PivotZone::try_from(zs).ok())
                        .collect::<Vec<_>>(),
                    self.buy_sell_points
                        .iter()
                        .map(|p| SignalPoint {
                            index: p.index as u32,
                            kind: p.kind as i32,
                            price: p.price,
                            label: p.label.clone(),
                        })
                        .collect::<Vec<_>>(),
                    segs.iter()
                        .filter_map(|s| s.to_proto(&finished_bis))
                        .collect::<Vec<_>>(),
                    segment_pivots
                        .iter()
                        .filter_map(|p| PivotZone::try_from(p).ok())
                        .collect::<Vec<_>>(),
                )
            } else {
                (
                    Vec::<FractalPoint>::new(),
                    Vec::<StrokeSegment>::new(),
                    Vec::<PivotZone>::new(),
                    Vec::<SignalPoint>::new(),
                    Vec::new(),
                    Vec::<PivotZone>::new(),
                )
            };
        if let Some(p) = profile {
            p.objects += t2.elapsed();
        }

        let params = self.settings.params();
        Ok(AnalysisSnapshot {
            meta: Some(SnapshotMeta {
                freq: self.settings.freq().to_string(),
                data_len: self.settings.request().data_len,
                input_hash: self.settings.request().input_hash,
                max_bi_num: self.settings.max_bi_num(),
                min_bi_len: self.settings.min_bi_len(),
                analysis_window: self.settings.meta_analysis_window(),
                generated_at_unix_ms,
                analyzer_version: params.czsc_version.clone(),
            }),
            channels,
            fractals: fractals_obj,
            strokes: strokes_obj,
            pivots: pivots_obj,
            signals: signals_obj,
            segments: segments_obj,
            segment_pivots: segment_pivots_obj,
        })
    }
}
