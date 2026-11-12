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
use crate::proto::analysis::{
    Direction as ProtoDirection, FractalMark, FractalPoint, PivotZone, Segment as ProtoSegment,
    StrokeSegment,
};

/// 对象无法对齐到原始 K 线下标（`fx_mid_index` 返回 None）。
///
/// 保持 `pub`：它作为 `impl TryFrom<&FX> for proto::analysis::FractalPoint`
/// 等实现的 `type Error`，经由公开 proto 类型对外可达。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlignError;

/// 分型在原始K线序列中的索引（通道对齐用）。
pub(crate) trait FxAnchor {
    fn mid_index(&self) -> Option<usize>;
}

impl FxAnchor for FX {
    fn mid_index(&self) -> Option<usize> {
        let mid = self.elements.get(1)?;
        Some(
            mid.elements
                .iter()
                .find(|bar| bar.dt == self.dt)
                .map_or(mid.id as usize, |bar| bar.id as usize),
        )
    }
}

/// 笔两端分型在原始K线序列中的索引。
pub(crate) trait BiAnchor {
    fn start_index(&self) -> Option<usize>;
    fn end_index(&self) -> Option<usize>;
}

impl BiAnchor for BI {
    fn start_index(&self) -> Option<usize> {
        self.fx_a.mid_index()
    }

    fn end_index(&self) -> Option<usize> {
        self.fx_b.mid_index()
    }
}

impl BiAnchor for Option<&BI> {
    fn start_index(&self) -> Option<usize> {
        self.and_then(|bi| bi.start_index())
    }

    fn end_index(&self) -> Option<usize> {
        self.and_then(|bi| bi.end_index())
    }
}

/// 中枢的绘制区间（通道对齐用）：优化后中枢整段绘制。
///
/// 中枢经 `build_pivots` 前端/后端优化后，首笔为进入笔剔除后的第一笔、
/// 末笔满足方向与间隔规则（离开笔落在相邻中枢之间）；这里从中枢第一笔
/// 起点画到最后一笔终点。相邻中枢之间留出连接笔间隔，304 通道的 3
/// （左右界共用）保留为防御性处理。
pub(crate) trait ZsAnchor {
    fn span(&self) -> Option<(usize, usize)>;
}

impl ZsAnchor for ZS {
    fn span(&self) -> Option<(usize, usize)> {
        let start = self.bis.first().start_index()?;
        let end = self.bis.last().end_index()?;
        Some((start.min(end), start.max(end)))
    }
}

impl From<CzscDirection> for ProtoDirection {
    fn from(d: CzscDirection) -> Self {
        match d {
            CzscDirection::Up => Self::Up,
            CzscDirection::Down => Self::Down,
        }
    }
}

impl TryFrom<&FX> for FractalPoint {
    type Error = AlignError;

    fn try_from(fx: &FX) -> Result<Self, Self::Error> {
        let index = fx.mid_index().ok_or(AlignError)?;
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
        let start_index = bi.start_index().ok_or(AlignError)?;
        let end_index = bi.end_index().ok_or(AlignError)?;

        Ok(StrokeSegment {
            start_index: start_index as u32,
            end_index: end_index as u32,
            direction: ProtoDirection::from(bi.direction) as i32,
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
        let (start_index, end_index) = zs.span().ok_or(AlignError)?;

        Ok(PivotZone {
            start_index: start_index as u32,
            end_index: end_index as u32,
            upper: zs.zg,
            lower: zs.zd,
            middle: zs.zz,
        })
    }
}

impl Segment {
    pub(crate) fn to_proto(&self, bis: &[BI]) -> Option<ProtoSegment> {
        let start_bi = bis.get(self.start_bi_idx)?;
        let end_bi = bis.get(self.end_bi_idx)?;
        let start_index = start_bi.start_index()?;
        let end_index = end_bi.end_index()?;

        Some(ProtoSegment {
            start_bi_index: self.start_bi_idx as u32,
            end_bi_index: self.end_bi_idx as u32,
            start_index: start_index as u32,
            end_index: end_index as u32,
            direction: ProtoDirection::from(self.direction) as i32,
            start_price: start_bi.fx_a.fx,
            end_price: end_bi.fx_b.fx,
            high: start_bi.get_high().max(end_bi.get_high()),
            low: start_bi.get_low().min(end_bi.get_low()),
            is_sure: self.is_sure,
        })
    }
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
}
