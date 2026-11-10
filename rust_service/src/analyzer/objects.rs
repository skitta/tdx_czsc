//! 通道对齐：分型/笔/中枢对象 → 原始K线下标与 proto 对象映射。
//!
//! 核心是 `fx_mid_index`：czsc-core 处理包含关系时（`remove_include`），
//! 合并后的 NewBar 的 `id` 锚定在合并组**第一根**原始K线，而其 `dt` 锚定
//! 在合并组的**极值**K线（向下合并取最低低点所在K线、向上合并取最高高点
//! 所在K线）；分型的 `fx.dt` 即中间元素的 `dt`，也就是极值K线时间。若直接
//! 取 `elements[1].id`，分型会被画到合并组起点，造成分型漂移（例：002340
//! 日线 2026-07 的底分型被画到 07-16 而非极值所在的 07-20）。因此这里在
//! 中间元素的原始K线里查找 `dt == fx.dt` 的那一根并取其 id；找不到时回退
//! 到 `elements[1].id`。
//!
//! proto 对象映射统一走标准转换 trait（`TryFrom`）：映射失败（对象缺少
//! 可对齐的 K 线下标）时返回 `AlignError`，调用方按"跳过该对象"处理。

use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction as CzscDirection;
use czsc_core::objects::fx::FX;
use czsc_core::objects::mark::Mark;
use czsc_core::objects::zs::ZS;

use crate::analyzer::segs::Segment;
use crate::analyzer::strict_pivots::{StrictPivot, seg_envelope};
use crate::proto::analysis::{
    Direction as ProtoDirection, FractalMark, FractalPoint, PivotZone, Segment as ProtoSegment,
    StrictPivotZone, StrokeSegment,
};

/// 对象无法对齐到原始 K 线下标（`fx_mid_index` 返回 None）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlignError;

impl std::fmt::Display for AlignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("对象无法对齐到原始 K 线下标")
    }
}

impl std::error::Error for AlignError {}

/// 分型在原始K线序列中的索引（通道对齐用）。
pub(crate) fn fx_mid_index(fx: &FX) -> Option<usize> {
    let mid = fx.elements.get(1)?;
    let id = mid
        .elements
        .iter()
        .find(|bar| bar.dt == fx.dt)
        .map_or(mid.id, |bar| bar.id);
    usize::try_from(id).ok()
}

/// 笔起点分型在原始K线序列中的索引。
pub(crate) fn bi_start_index(bi: Option<&BI>) -> Option<usize> {
    bi.and_then(|bi| fx_mid_index(&bi.fx_a))
}

/// 笔终点分型在原始K线序列中的索引。
pub(crate) fn bi_end_index(bi: Option<&BI>) -> Option<usize> {
    bi.and_then(|bi| fx_mid_index(&bi.fx_b))
}

/// 中枢的绘制区间（通道对齐用）：优化后中枢整段绘制。
///
/// 中枢经 `build_pivots` 前端/后端优化后，首笔为进入笔剔除后的第一笔、
/// 末笔满足方向与间隔规则（离开笔落在相邻中枢之间）；这里从中枢第一笔
/// 起点画到最后一笔终点。相邻中枢之间留出连接笔间隔，304 通道的 3
/// （左右界共用）保留为防御性处理。
pub(crate) fn zs_span(zs: &ZS) -> Option<(usize, usize)> {
    let start = bi_start_index(zs.bis.first())?;
    let end = bi_end_index(zs.bis.last())?;
    Some((start.min(end), start.max(end)))
}

fn proto_direction(d: CzscDirection) -> i32 {
    match d {
        CzscDirection::Up => ProtoDirection::Up as i32,
        CzscDirection::Down => ProtoDirection::Down as i32,
    }
}

impl TryFrom<&FX> for FractalPoint {
    type Error = AlignError;

    fn try_from(fx: &FX) -> Result<Self, Self::Error> {
        let index = fx_mid_index(fx).ok_or(AlignError)?;
        let mark = match fx.mark {
            Mark::G => FractalMark::FractalTop as i32,
            Mark::D => FractalMark::FractalBottom as i32,
        };

        Ok(FractalPoint {
            index: index as u32,
            mark,
            price: fx.fx,
            high: fx.high,
            low: fx.low,
        })
    }
}

impl TryFrom<&BI> for StrokeSegment {
    type Error = AlignError;

    fn try_from(bi: &BI) -> Result<Self, Self::Error> {
        let start_index = fx_mid_index(&bi.fx_a).ok_or(AlignError)?;
        let end_index = fx_mid_index(&bi.fx_b).ok_or(AlignError)?;

        Ok(StrokeSegment {
            start_index: start_index as u32,
            end_index: end_index as u32,
            direction: proto_direction(bi.direction),
            start_price: bi.fx_a.fx,
            end_price: bi.fx_b.fx,
            high: bi.get_high(),
            low: bi.get_low(),
        })
    }
}

impl TryFrom<&ZS> for PivotZone {
    type Error = AlignError;

    fn try_from(zs: &ZS) -> Result<Self, Self::Error> {
        // 优化后中枢整段绘制（含前端优化后的首笔；离开笔在相邻中枢之间）
        let (start_index, end_index) = zs_span(zs).ok_or(AlignError)?;

        Ok(PivotZone {
            start_index: start_index as u32,
            end_index: end_index as u32,
            upper: zs.zg,
            lower: zs.zd,
            middle: zs.zz,
        })
    }
}

impl TryFrom<&StrictPivot> for StrictPivotZone {
    type Error = AlignError;

    fn try_from(p: &StrictPivot) -> Result<Self, Self::Error> {
        Ok(StrictPivotZone {
            start_index: p.start_index as u32,
            end_index: p.end_index as u32,
            upper: p.upper,
            lower: p.lower,
            middle: p.middle,
            gg: p.gg,
            dd: p.dd,
            level: p.level,
            is_sure: p.is_sure,
            move_count: p.move_count as u32,
            kind: p.kind.as_i32(),
        })
    }
}

/// 段 → proto 对象（需要根据段索引回查笔序列，保持自由函数形式）。
pub(crate) fn map_segment(seg: &Segment, bis: &[BI]) -> Option<ProtoSegment> {
    let start_bi = bis.get(seg.start_bi_idx)?;
    let end_bi = bis.get(seg.end_bi_idx)?;
    let start_index = fx_mid_index(&start_bi.fx_a)?;
    let end_index = fx_mid_index(&end_bi.fx_b)?;
    let (high, low) = seg_envelope(bis, seg)?;

    Some(ProtoSegment {
        start_bi_index: seg.start_bi_idx as u32,
        end_bi_index: seg.end_bi_idx as u32,
        start_index: start_index as u32,
        end_index: end_index as u32,
        direction: proto_direction(seg.direction),
        start_price: start_bi.fx_a.fx,
        end_price: end_bi.fx_b.fx,
        high,
        low,
        is_sure: seg.is_sure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::fixtures::make_series_bi;

    /// 分型映射：夹具中每个分型都带 elements，`fx_mid_index` 应解析出
    /// 夹具编码的 bar id（fx_a -> idx、fx_b -> idx+1）。
    #[test]
    fn try_from_bi_yields_aligned_stroke() {
        let bi = make_series_bi(3, 10.0, 20.0);
        let stroke = StrokeSegment::try_from(&bi).expect("aligned bi");
        assert_eq!(stroke.start_index, 3);
        assert_eq!(stroke.end_index, 4);
        assert_eq!(stroke.start_price, 10.0);
        assert_eq!(stroke.end_price, 20.0);
    }

    /// 映射失败（无 elements）→ AlignError，调用方按跳过处理。
    #[test]
    fn try_from_bi_fails_without_elements() {
        let mut bi = make_series_bi(0, 10.0, 20.0);
        bi.fx_a.elements.clear();
        assert_eq!(StrokeSegment::try_from(&bi), Err(AlignError));
    }

    /// 段对象高低必须取段内全部笔的包络，而不是只看首末笔。
    #[test]
    fn map_segment_uses_full_envelope() {
        let points = [20.0, 10.0, 30.0, 5.0, 25.0, 15.0];
        let bis: Vec<BI> = points
            .windows(2)
            .enumerate()
            .map(|(i, w)| make_series_bi(i as i64, w[0], w[1]))
            .collect();
        let segment = Segment {
            start_bi_idx: 0,
            end_bi_idx: bis.len() - 1,
            direction: CzscDirection::Down,
            is_sure: true,
        };
        let object = map_segment(&segment, &bis).expect("segment object");
        assert_eq!(object.high, 30.0);
        assert_eq!(object.low, 5.0);
    }
}
