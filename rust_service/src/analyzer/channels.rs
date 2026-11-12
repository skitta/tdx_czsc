//! 输出通道构建：func_id → 对齐 DataLen 的 float[] 值序列。
//!
//! [`CHANNEL_SPECS`] 是通道的唯一事实来源（编号、类型、诊断名、填充策略），
//! [`ChannelBuilder`] 是唯一入口：解析出表项后按填充策略分派到对应填充器。
//! 分型/笔/中枢/段/信号各通道的填充器只做"对象 → 价格/方向值"的写入，
//! 不承担算法逻辑。

use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction as CzscDirection;
use czsc_core::objects::fx::FX;
use czsc_core::objects::mark::Mark;
use czsc_core::objects::zs::ZS;

use super::error::AnalysisError;
use super::objects::{FxAnchor, ZsAnchor};
use super::pivots::SegmentPivot;
use super::segs::Segment;
use super::signals::BuySellPoint;
use crate::proto::analysis::{ChannelKind, ChannelSeries};

/// 已校验的输出通道号，携带解析出的 [`ChannelSpec`]。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelFuncId {
    id: u32,
    spec: &'static ChannelSpec,
}

impl TryFrom<u32> for ChannelFuncId {
    type Error = AnalysisError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        let spec = CHANNEL_SPECS
            .iter()
            .find(|spec| spec.func_id == value)
            .ok_or_else(|| Self::unsupported(value))?;
        Ok(Self { id: value, spec })
    }
}

impl ChannelFuncId {
    fn unsupported(func_id: u32) -> AnalysisError {
        AnalysisError::invalid_request(format!("unsupported func_id: {func_id}"))
    }
}

/// 输出通道的完整定义：编号、类型、诊断名与填充策略。
#[derive(Debug)]
pub struct ChannelSpec {
    func_id: u32,
    kind: ChannelKind,
    name: &'static str,
    fill: Fill,
}

impl ChannelSpec {
    /// 通道编号（即 DLL 侧请求的 func_id）。
    pub fn func_id(&self) -> u32 {
        self.func_id
    }

    /// 通道诊断名（仅用于日志与离线输出，不进入协议语义）。
    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// 通道填充策略：[`ChannelBuilder::build`] 按此分派到对应填充器。
#[derive(Debug)]
enum Fill {
    /// 顶/底分型极值K线处的分型价格（101/102）。
    Fractal(Mark),
    /// 每笔起止分型极值K线处的价格（201）。
    BiPoints,
    /// 笔覆盖区间填 ±1（202）。
    BiDirection,
    /// 已确认段端点的分型价格（203）。
    SegPoints,
    /// 已确认段覆盖区间填 ±1（204）。
    SegDirection,
    /// 中枢绘制区间填 `zg`/`zd`/`zz`（301~303）。
    Zs(ZsField),
    /// 中枢左右界标记 1/2/3（304）。
    ZsBoundary,
    /// 段中枢绘制区间填 `zg`/`zd`（305~306）。
    SegmentZs(ZsField),
    /// 段中枢左右界标记 1/2/3（307）。
    SegmentZsBoundary,
    /// 买卖点锚点价格，按 func_id 过滤（401~406）。
    Signal,
    /// 诊断通道，由 handlers 侧补丁填充（901~903）。
    Debug,
}

/// 全量支持的通道表（唯一事实来源，按 func_id 升序）。
///
/// `handlers/version.rs` 的 `supported_func_ids`、DLL 侧约定与离线示例都以此
/// 为准，统一通过 [`ChannelBuilder::specs`] 访问：新增通道只需在此加一行；
/// 引入全新填充行为时，`build` 的 `Fill` 分派会因未覆盖而编译失败。
const CHANNEL_SPECS: &[ChannelSpec] = &[
    ChannelSpec {
        func_id: 101,
        kind: ChannelKind::ChannelFxTop,
        name: "fx_top",
        fill: Fill::Fractal(Mark::G),
    },
    ChannelSpec {
        func_id: 102,
        kind: ChannelKind::ChannelFxBottom,
        name: "fx_bottom",
        fill: Fill::Fractal(Mark::D),
    },
    ChannelSpec {
        func_id: 201,
        kind: ChannelKind::ChannelBiPoints,
        name: "bi_points",
        fill: Fill::BiPoints,
    },
    ChannelSpec {
        func_id: 202,
        kind: ChannelKind::ChannelBiDirection,
        name: "bi_direction",
        fill: Fill::BiDirection,
    },
    ChannelSpec {
        func_id: 203,
        kind: ChannelKind::ChannelSegPoints,
        name: "seg_points",
        fill: Fill::SegPoints,
    },
    ChannelSpec {
        func_id: 204,
        kind: ChannelKind::ChannelSegDirection,
        name: "seg_direction",
        fill: Fill::SegDirection,
    },
    ChannelSpec {
        func_id: 301,
        kind: ChannelKind::ChannelZsUpper,
        name: "zs_upper",
        fill: Fill::Zs(ZsField::Upper),
    },
    ChannelSpec {
        func_id: 302,
        kind: ChannelKind::ChannelZsLower,
        name: "zs_lower",
        fill: Fill::Zs(ZsField::Lower),
    },
    ChannelSpec {
        func_id: 303,
        kind: ChannelKind::ChannelZsMiddle,
        name: "zs_middle",
        fill: Fill::Zs(ZsField::Middle),
    },
    ChannelSpec {
        func_id: 304,
        kind: ChannelKind::ChannelZsBoundary,
        name: "zs_boundary",
        fill: Fill::ZsBoundary,
    },
    ChannelSpec {
        func_id: 305,
        kind: ChannelKind::ChannelSegZsUpper,
        name: "seg_zs_upper",
        fill: Fill::SegmentZs(ZsField::Upper),
    },
    ChannelSpec {
        func_id: 306,
        kind: ChannelKind::ChannelSegZsLower,
        name: "seg_zs_lower",
        fill: Fill::SegmentZs(ZsField::Lower),
    },
    ChannelSpec {
        func_id: 307,
        kind: ChannelKind::ChannelSegZsBoundary,
        name: "seg_zs_boundary",
        fill: Fill::SegmentZsBoundary,
    },
    ChannelSpec {
        func_id: 401,
        kind: ChannelKind::ChannelBuy1,
        name: "buy1",
        fill: Fill::Signal,
    },
    ChannelSpec {
        func_id: 402,
        kind: ChannelKind::ChannelSell1,
        name: "sell1",
        fill: Fill::Signal,
    },
    ChannelSpec {
        func_id: 403,
        kind: ChannelKind::ChannelBuy2,
        name: "buy2",
        fill: Fill::Signal,
    },
    ChannelSpec {
        func_id: 404,
        kind: ChannelKind::ChannelSell2,
        name: "sell2",
        fill: Fill::Signal,
    },
    ChannelSpec {
        func_id: 405,
        kind: ChannelKind::ChannelBuy3,
        name: "buy3",
        fill: Fill::Signal,
    },
    ChannelSpec {
        func_id: 406,
        kind: ChannelKind::ChannelSell3,
        name: "sell3",
        fill: Fill::Signal,
    },
    ChannelSpec {
        func_id: 901,
        kind: ChannelKind::ChannelDebugCacheHit,
        name: "debug_cache_hit",
        fill: Fill::Debug,
    },
    ChannelSpec {
        func_id: 902,
        kind: ChannelKind::ChannelDebugServiceReady,
        name: "debug_service_ready",
        fill: Fill::Debug,
    },
    ChannelSpec {
        func_id: 903,
        kind: ChannelKind::ChannelDebugLatencyMs,
        name: "debug_latency_ms",
        fill: Fill::Debug,
    },
];

/// 通道构建器：对象输入通过链式 setter 注入，`build`/`build_all` 负责分派。
///
/// 对 crate 外只暴露 [`ChannelBuilder::specs`]（通道表查询）；装配与构建
/// 属于分析内部流程，可见性收在 `pub(crate)`。
pub struct ChannelBuilder<'a> {
    data_len: usize,
    fractals: &'a [FX],
    finished_bis: &'a [BI],
    pivots: &'a [ZS],
    segment_pivots: &'a [SegmentPivot],
    segs: &'a [Segment],
    buy_sell_points: &'a [BuySellPoint],
}

impl<'a> ChannelBuilder<'a> {
    pub(crate) fn new(data_len: usize) -> Self {
        Self {
            data_len,
            fractals: &[],
            finished_bis: &[],
            pivots: &[],
            segment_pivots: &[],
            segs: &[],
            buy_sell_points: &[],
        }
    }

    pub(crate) fn fractals(mut self, fractals: &'a [FX]) -> Self {
        self.fractals = fractals;
        self
    }

    pub(crate) fn bis(mut self, finished_bis: &'a [BI]) -> Self {
        self.finished_bis = finished_bis;
        self
    }

    pub(crate) fn pivots(mut self, pivots: &'a [ZS]) -> Self {
        self.pivots = pivots;
        self
    }

    pub(crate) fn segment_pivots(mut self, segment_pivots: &'a [SegmentPivot]) -> Self {
        self.segment_pivots = segment_pivots;
        self
    }

    pub(crate) fn segs(mut self, segs: &'a [Segment]) -> Self {
        self.segs = segs;
        self
    }

    pub(crate) fn buy_sell_points(mut self, buy_sell_points: &'a [BuySellPoint]) -> Self {
        self.buy_sell_points = buy_sell_points;
        self
    }

    /// 构建单个输出通道：func_id → 表项（类型/名称/填充策略），并填充值序列。
    pub(crate) fn build(&self, func_id: u32) -> Result<ChannelSeries, AnalysisError> {
        let func_id = ChannelFuncId::try_from(func_id)?;
        let mut values = vec![0.0; self.data_len];
        match &func_id.spec.fill {
            Fill::Fractal(mark) => self.fill_fractal_channel(&mut values, mark)?,
            Fill::BiPoints => self.fill_bi_points_channel(&mut values)?,
            Fill::BiDirection => self.fill_bi_direction_channel(&mut values)?,
            Fill::SegPoints => self.fill_seg_points_channel(&mut values)?,
            Fill::SegDirection => self.fill_seg_direction_channel(&mut values)?,
            Fill::Zs(field) => self.fill_zs_channel(&mut values, *field)?,
            Fill::ZsBoundary => self.fill_zs_boundary_channel(&mut values)?,
            Fill::SegmentZs(field) => self.fill_segment_zs_channel(&mut values, *field)?,
            Fill::SegmentZsBoundary => self.fill_segment_zs_boundary_channel(&mut values)?,
            Fill::Signal => self.fill_signal_channel(&mut values, func_id.id)?,
            Fill::Debug => {}
        }
        Ok(ChannelSeries {
            func_id: func_id.id,
            kind: func_id.spec.kind as i32,
            name: func_id.spec.name.to_owned(),
            values,
        })
    }

    /// 按输入顺序构建多个通道。
    pub(crate) fn build_all(&self, func_ids: &[u32]) -> Result<Vec<ChannelSeries>, AnalysisError> {
        func_ids.iter().map(|id| self.build(*id)).collect()
    }

    /// 全量通道表（唯一事实来源，按 func_id 升序）。
    pub fn specs() -> &'static [ChannelSpec] {
        CHANNEL_SPECS
    }

    fn fill_fractal_channel(&self, values: &mut [f32], mark: &Mark) -> Result<(), AnalysisError> {
        let mut values = ChannelBuf::new(values);
        for fx in self.fractals.iter().filter(|fx| fx.mark == *mark) {
            if let Some(index) = fx.mid_index() {
                values.set(index, fx.fx as f32, "fractal")?;
            }
        }
        Ok(())
    }

    fn fill_bi_points_channel(&self, values: &mut [f32]) -> Result<(), AnalysisError> {
        let mut values = ChannelBuf::new(values);
        for bi in self.finished_bis {
            // 每笔的起止分型都标注（此前只填终点 fx_b，第一笔起点缺失）
            if let Some(index) = bi.fx_a.mid_index() {
                values.set(index, bi.fx_a.fx as f32, "bi start")?;
            }
            if let Some(index) = bi.fx_b.mid_index() {
                values.set(index, bi.fx_b.fx as f32, "bi end")?;
            }
        }
        Ok(())
    }

    fn fill_bi_direction_channel(&self, values: &mut [f32]) -> Result<(), AnalysisError> {
        let mut values = ChannelBuf::new(values);
        for bi in self.finished_bis {
            let Some(start) = bi.fx_a.mid_index() else {
                continue;
            };
            let Some(end) = bi.fx_b.mid_index() else {
                continue;
            };

            let direction = match bi.direction {
                CzscDirection::Up => 1.0,
                CzscDirection::Down => -1.0,
            };

            let (lower, upper) = (start.min(end), start.max(end));
            values.fill_range(lower, upper, direction, "bi direction")?;
        }
        Ok(())
    }

    /// 段端点通道（203）：段首笔起点分型与末笔终点分型处输出分型价格。
    ///
    /// **只输出已确认段**（`is_sure=true`）；尾部未完成/暂定段不显示（图表上只画
    /// 特征序列分型确认的段）。段的起止都是笔端点（分型位置），方向由 101/102
    /// 顶底分型通道判定：向上段起点为底分型、终点为顶分型；向下段反之。相邻段
    /// 共享端点分型，重复写值幂等。
    fn fill_seg_points_channel(&self, values: &mut [f32]) -> Result<(), AnalysisError> {
        let mut values = ChannelBuf::new(values);
        for seg in self.segs.iter().filter(|s| s.is_sure) {
            if let Some(start_bi) = self.finished_bis.get(seg.start_bi_idx)
                && let Some(index) = start_bi.fx_a.mid_index()
            {
                values.set(index, start_bi.fx_a.fx as f32, "seg start")?;
            }
            if let Some(end_bi) = self.finished_bis.get(seg.end_bi_idx)
                && let Some(index) = end_bi.fx_b.mid_index()
            {
                values.set(index, end_bi.fx_b.fx as f32, "seg end")?;
            }
        }
        Ok(())
    }

    /// 段方向通道（204）：已确认段覆盖区间填 ±1（向上=1，向下=-1），
    /// 与 202 笔向通道对称；未确认段不填。
    fn fill_seg_direction_channel(&self, values: &mut [f32]) -> Result<(), AnalysisError> {
        let mut values = ChannelBuf::new(values);
        for seg in self.segs.iter().filter(|s| s.is_sure) {
            let Some(start_bi) = self.finished_bis.get(seg.start_bi_idx) else {
                continue;
            };
            let Some(end_bi) = self.finished_bis.get(seg.end_bi_idx) else {
                continue;
            };
            let Some(start) = start_bi.fx_a.mid_index() else {
                continue;
            };
            let Some(end) = end_bi.fx_b.mid_index() else {
                continue;
            };

            let direction = match seg.direction {
                CzscDirection::Up => 1.0,
                CzscDirection::Down => -1.0,
            };
            let (lower, upper) = (start.min(end), start.max(end));
            values.fill_range(lower, upper, direction, "seg direction")?;
        }
        Ok(())
    }

    /// 中枢几何写入：`pivots` 逐个产出（绘制区间, zg, zd, zz）。
    fn fill_pivot_field(
        &self,
        values: &mut [f32],
        field: ZsField,
        pivots: impl Iterator<Item = ((usize, usize), f64, f64, f64)>,
        channel: &str,
    ) -> Result<(), AnalysisError> {
        let mut values = ChannelBuf::new(values);
        for ((start, end), zg, zd, zz) in pivots {
            let pivot_value = match field {
                ZsField::Upper => zg,
                ZsField::Lower => zd,
                ZsField::Middle => zz,
            } as f32;
            values.fill_range(start, end, pivot_value, channel)?;
        }
        Ok(())
    }

    fn fill_zs_channel(&self, values: &mut [f32], field: ZsField) -> Result<(), AnalysisError> {
        self.fill_pivot_field(
            values,
            field,
            self.pivots
                .iter()
                .filter_map(|zs| zs.span().map(|span| (span, zs.zg, zs.zd, zs.zz))),
            "zs",
        )
    }

    /// 中枢边界标记：每个中枢绘制区间首 K 线输出 1（左界）、尾 K 线输出 2（右界），
    /// 其余为 0；既是某中枢右界又是另一中枢左界的共享 K 线输出 3（左右界共用）。
    fn fill_zs_boundary_channel(&self, values: &mut [f32]) -> Result<(), AnalysisError> {
        self.fill_pivot_boundary(values, self.pivots.iter().filter_map(|zs| zs.span()))
    }

    fn fill_segment_zs_channel(
        &self,
        values: &mut [f32],
        field: ZsField,
    ) -> Result<(), AnalysisError> {
        self.fill_pivot_field(
            values,
            field,
            self.segment_pivots
                .iter()
                .filter_map(|p| p.span().map(|span| (span, p.zg, p.zd, p.zz))),
            "seg zs",
        )
    }

    fn fill_segment_zs_boundary_channel(&self, values: &mut [f32]) -> Result<(), AnalysisError> {
        self.fill_pivot_boundary(values, self.segment_pivots.iter().filter_map(|p| p.span()))
    }

    /// 中枢左右界标记共用实现（304/307）。
    fn fill_pivot_boundary(
        &self,
        values: &mut [f32],
        spans: impl Iterator<Item = (usize, usize)>,
    ) -> Result<(), AnalysisError> {
        let mut values = ChannelBuf::new(values);
        let mut starts = Vec::new();
        let mut ends = Vec::new();
        for (start, end) in spans {
            starts.push(start);
            ends.push(end);
        }

        for &start in &starts {
            values.set_if_in_bounds(start, 1.0);
        }
        for &end in &ends {
            if let Some(current) = values.get(end) {
                values.set_if_in_bounds(end, if current > 0.5 { 3.0 } else { 2.0 });
            }
        }
        Ok(())
    }

    fn fill_signal_channel(&self, values: &mut [f32], func_id: u32) -> Result<(), AnalysisError> {
        let mut values = ChannelBuf::new(values);
        for point in self.buy_sell_points.iter().filter(|p| p.func_id == func_id) {
            values.set(point.index, point.price as f32, "signal")?;
        }
        Ok(())
    }
}

/// 输出通道的零成本可写视图：集中处理越界错误，避免各填充器重复索引检查。
#[repr(transparent)]
struct ChannelBuf<'a>(&'a mut [f32]);

impl<'a> ChannelBuf<'a> {
    fn new(values: &'a mut [f32]) -> Self {
        Self(values)
    }

    fn set(&mut self, index: usize, value: f32, channel: &str) -> Result<(), AnalysisError> {
        let Some(slot) = self.0.get_mut(index) else {
            return Err(Self::out_of_range(channel, index, self.0.len()));
        };
        *slot = value;
        Ok(())
    }

    fn fill_range(
        &mut self,
        lower: usize,
        upper: usize,
        value: f32,
        channel: &str,
    ) -> Result<(), AnalysisError> {
        if lower > upper || upper >= self.0.len() {
            return Err(Self::out_of_range(channel, upper, self.0.len()));
        }
        self.0[lower..=upper].fill(value);
        Ok(())
    }

    fn get(&self, index: usize) -> Option<f32> {
        self.0.get(index).copied()
    }

    fn set_if_in_bounds(&mut self, index: usize, value: f32) {
        if let Some(slot) = self.0.get_mut(index) {
            *slot = value;
        }
    }

    fn out_of_range(channel: &str, index: usize, len: usize) -> AnalysisError {
        AnalysisError::invalid_request(format!(
            "{channel} index {index} out of range for channel length {len}"
        ))
    }
}

#[derive(Debug, Clone, Copy)]
enum ZsField {
    Upper,
    Lower,
    Middle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_specs_are_sorted_unique_and_resolvable() {
        for pair in CHANNEL_SPECS.windows(2) {
            assert!(pair[0].func_id < pair[1].func_id, "通道表需按 func_id 升序");
        }
        for spec in CHANNEL_SPECS {
            let resolved = ChannelFuncId::try_from(spec.func_id).expect("表项应可解析");
            assert_eq!(resolved.spec.func_id, spec.func_id);
            assert_eq!(resolved.spec.kind, spec.kind);
        }
        assert!(ChannelFuncId::try_from(999).is_err());
    }

    #[test]
    fn empty_channel_has_zeros() {
        let ch = ChannelBuilder::new(10).build(101).expect("channel");
        assert_eq!(ch.func_id, 101);
        assert!(ch.values.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn build_all_preserves_request_order() {
        let channels = ChannelBuilder::new(10)
            .build_all(&[304, 101, 302])
            .expect("channels");
        assert_eq!(
            channels.iter().map(|ch| ch.func_id).collect::<Vec<_>>(),
            vec![304, 101, 302]
        );
    }

    #[test]
    fn build_all_empty_request_returns_empty() {
        let channels = ChannelBuilder::new(10)
            .build_all(&[])
            .expect("empty request");
        assert!(channels.is_empty());
    }

    #[test]
    fn unknown_func_id_keeps_error_text() {
        let err = ChannelBuilder::new(10).build(999).expect_err("unknown id");
        assert_eq!(err.to_string(), "invalid request: unsupported func_id: 999");
    }
}
