//! 中枢构建：笔中枢与段中枢共用同一套原生贪心划分 + 前端/后端优化。
//!
//! 构件通过 [`PivotUnit`] 抽象为"方向 + 高低点 + 绘制区间 + 起点价格"：笔和
//! 已确认段只是构造 unit 的来源不同，算法本身（原生归属、趋势判定、进入/
//! 离开构件剔除、相邻间隔）完全复用。
//!
//! **原生划分**（对齐 `czsc-core::get_zs_seq`）：逐构件贪心归属，构件完全
//! 离开当前中枢区间（向上构件的 high 低于下沿、向下构件的 low 高于上沿）时
//! 另起中枢；与当前中枢区间无交集的"离开/连接构件"会并入下一个中枢作为其
//! 第一构件，导致中枢首/末构件可能混入进入/连接构件。这里保留原生的中枢
//! 划分，优化两端：
//!
//! **前端（第一构件）**：中枢第一构件必须与行情趋势相反——上升行情中第一
//! 构件向下（"下上下"重叠）、下跌行情中第一构件向上（"上下上"重叠）。行情方向
//! 按中枢高低位置判定：后一个中枢中轴（zz）比前一个高 → 上升行情，更低 →
//! 下降行情；中轴平齐沿用前一趋势。第一个中枢无前序中枢，以其位置与窗口
//! 起点价格比较兜底。首构件与趋势同向视为进入/连接构件，予以剔除；区间
//! [zd, zg] 由剔除后剩余前 3 个构件的重叠重新计算。
//!
//! **后端（最后一构件）**：中枢最后一构件必须与行情趋势相反（与前端对称，
//! 中枢为奇数构件结构）——末构件与趋势同向视为离开构件，予以剔除。
//!
//! **相邻间隔**：前一个中枢后端与后一中枢前端不得共享端点，必须至少间隔
//! 一构件——后端剔除后，离开/连接构件落在两个中枢矩形之间，不属于任一中枢。
//!
//! **延伸**：中枢存续期间，只要后续构件与 `[zd, zg]` 仍有交集就继续并入；
//! 构件完全离开区间时延伸结束——与三买/三卖（回抽不再回到中枢）同一口径。
//!
//! **扩展与升级（九段 + 三三得九）**：中枢构件数达到九段时并入高一级中枢；
//! 相邻且区间重叠的中枢先串成一条链，链首到链尾的构件跨度达到九段时整条链
//! 一起升级（扩展）。升级把每 3 个构件合成一个次级别走势（区间取 3 个构件的
//! 包络，不足 3 个的尾巴并入末段），高一级区间取前 3 个次级别走势的交集，
//! 再与全部子中枢的区间取包络——**高一级中枢在时间与价格上都必须包住子中枢**，
//! 左右和上下都不会缩窄；交集为空则不升级，保持原级别。升级后的中枢
//! **替换**其子中枢——只画一个大矩形，绘制区间覆盖整条链。只做一级升级
//! （level ≤ 2）。
//!
//! 剔除后不足 3 个构件、前 3 个构件无重叠、或无法同时满足上述规则的候选中枢整组
//! 剔除（不绘制）。

use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction as CzscDirection;
use czsc_core::objects::zs::ZS;
use std::collections::HashSet;

use super::objects::BiAnchor;
use super::segs::Segment;
use super::signals::BuySellPoint;

/// 中枢价格区间 `[zd, zg]`：`zd` 为下沿、`zg` 为上沿。
///
/// 区间交集、包络、相交与中轴都挂在这个类型上：`intersect` 与 `hull` 互为
/// 对偶（三三得九取前 3 个次级别走势的 `intersect`，包络取全部子中枢的
/// `hull`），避免用裸 `(f64, f64)` 在多个函数间传递同一语义。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Band {
    pub(crate) zd: f64,
    pub(crate) zg: f64,
}

impl Band {
    /// 交集单位元（全集）：`band.intersect(Band::UNIVERSE) == band`。
    const UNIVERSE: Self = Self {
        zd: f64::NEG_INFINITY,
        zg: f64::INFINITY,
    };

    /// 包络单位元（空集）：`band.hull(Band::EMPTY) == band`。
    const EMPTY: Self = Self {
        zd: f64::INFINITY,
        zg: f64::NEG_INFINITY,
    };

    /// 构件自身的价格区间（`zd` = low、`zg` = high）。
    fn of(unit: &PivotUnit) -> Self {
        Self {
            zd: unit.low,
            zg: unit.high,
        }
    }

    /// 区间是否非空（`zg >= zd`）。
    fn is_valid(self) -> bool {
        self.zg >= self.zd
    }

    /// 交集：`zd` 取高者、`zg` 取低者；可能为空，用 [`Self::is_valid`] 判断。
    fn intersect(self, other: Self) -> Self {
        Self {
            zd: self.zd.max(other.zd),
            zg: self.zg.min(other.zg),
        }
    }

    /// 包络：`zd` 取低者、`zg` 取高者（只加宽，不收窄）。
    fn hull(self, other: Self) -> Self {
        Self {
            zd: self.zd.min(other.zd),
            zg: self.zg.max(other.zg),
        }
    }

    /// 闭区间是否相交。
    fn overlaps(self, other: Self) -> bool {
        self.zg >= other.zd && other.zg >= self.zd
    }

    /// 中轴：区间中点。
    pub(crate) fn mid(self) -> f64 {
        self.zd + (self.zg - self.zd) * 0.5
    }
}

/// 中枢级别：本级（3~8 个构件）或九段升级后的高一级中枢。
///
/// 级别只在本级/高一级之间二选一（不做多级递归），`code` 是 308/309
/// 级别通道的协议取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PivotLevel {
    /// 本级中枢。
    Base,
    /// 九段升级后的高一级中枢。
    Upgraded,
}

impl PivotLevel {
    /// 级别通道取值：本级 = 1，升级 = 2。
    pub(crate) fn code(self) -> f32 {
        match self {
            Self::Base => 1.0,
            Self::Upgraded => 2.0,
        }
    }
}

/// 中枢构件（笔或已确认段）的算法视图：只保留中枢判定实际读取的字段。
///
/// 笔与段的价格语义一致：`high`/`low` 为构件最高/最低价，`start_price` 为
/// 构件起点价（仅用于首个中枢的趋势兜底），`start_index`/`end_index` 为
/// 构件两端对应的原始 K 线下标（缺失时该中枢不参与通道绘制）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PivotUnit {
    direction: CzscDirection,
    high: f64,
    low: f64,
    start_index: Option<usize>,
    end_index: Option<usize>,
    start_price: f64,
}

impl PivotUnit {
    /// 中枢最少构件数：三段重叠才构成中枢。
    const MIN_PIVOT_UNITS: usize = 3;

    /// 升级门槛：中枢（或中枢链）构件跨度达到九段才并入高一级中枢。
    const MIN_UPGRADE_UNITS: usize = 9;

    /// 每个次级别走势由 3 个构件合成。
    const SUBTREND_UNITS: usize = 3;

    /// 高一级中枢由前 3 个次级别走势重叠而成（三三得九）。
    const UPGRADE_TRENDS: usize = 3;

    /// 以一笔为构件。
    fn from_bi(bi: &BI) -> Self {
        Self {
            direction: bi.direction,
            high: bi.get_high(),
            low: bi.get_low(),
            start_index: bi.start_index(),
            end_index: bi.end_index(),
            start_price: bi.fx_a.fx,
        }
    }

    /// 一组构件前 3 个的重叠区间：`zd` 取低点最大值、`zg` 取高点最小值。
    fn band(units: &[PivotUnit], indices: &[usize]) -> Band {
        indices
            .iter()
            .take(Self::SUBTREND_UNITS)
            .map(|&index| Band::of(&units[index]))
            .fold(Band::UNIVERSE, Band::intersect)
    }

    /// `first..=last` 构件的包络区间（最低 low、最高 high）。
    fn envelope(units: &[PivotUnit], first: usize, last: usize) -> Band {
        units
            .get(first..=last)
            .into_iter()
            .flatten()
            .map(Band::of)
            .fold(Band::EMPTY, Band::hull)
    }

    /// 三三得九：每 3 个构件合成一个次级别走势（取包络），不足 3 个的尾巴
    /// 并入末段；构件不足九段返回 `None`。
    fn subtrends(units: &[PivotUnit], first: usize, last: usize) -> Option<Vec<Band>> {
        let count = last.checked_sub(first)? + 1;
        if count < Self::MIN_UPGRADE_UNITS {
            return None;
        }
        // 段数按 3 取整；余下的尾巴由末段吸收，不另起一段
        let trends = count / Self::SUBTREND_UNITS;
        Some(
            (0..trends)
                .map(|index| {
                    let start = first + index * Self::SUBTREND_UNITS;
                    // 末段一直取到 `last`，把不足 3 个的尾巴并入末段
                    let end = if index + 1 == trends {
                        last
                    } else {
                        start + Self::SUBTREND_UNITS - 1
                    };
                    Self::envelope(units, start, end)
                })
                .collect(),
        )
    }

    /// 高一级中枢区间：前 3 个次级别走势的交集；构件不足九段或交集为空返回
    /// `None`（交由调用方保持原级别）。
    fn upgrade_band(units: &[PivotUnit], first: usize, last: usize) -> Option<Band> {
        let band = Self::subtrends(units, first, last)?
            .into_iter()
            .take(Self::UPGRADE_TRENDS)
            .reduce(Band::intersect)?;
        band.is_valid().then_some(band)
    }

    /// 构件是否仍与中枢区间有交（未完全离开）：向上构件的高点不破下沿、
    /// 向下构件的低点不破上沿。
    fn stays_in(band: Band, unit: &PivotUnit) -> bool {
        match unit.direction {
            CzscDirection::Up => unit.high >= band.zd,
            CzscDirection::Down => unit.low <= band.zg,
        }
    }
}

/// 一个待绘制中枢：构件下标 + 可选的高一级区间。
///
/// `upgrade == None` 表示维持 level-1，区间取前 3 个构件的重叠；
/// `upgrade == Some(band)` 表示已按九段规则升级，区间取三三得九的前 3 个
/// 次级别走势交集与全部子中枢区间的包络，绘制时覆盖整条链（替换被合并的
/// 子中枢）。
#[derive(Debug)]
pub(crate) struct PivotSelection {
    indices: Vec<usize>,
    upgrade: Option<Band>,
}

impl PivotSelection {
    /// 中枢最终区间：升级后取高一级区间，否则取前 3 个构件的重叠。
    #[must_use]
    pub(crate) fn band(&self, units: &[PivotUnit]) -> Band {
        self.upgrade
            .unwrap_or_else(|| PivotUnit::band(units, &self.indices))
    }

    /// 中枢级别：本级或九段升级后的高一级中枢。
    #[must_use]
    pub(crate) fn level(&self) -> PivotLevel {
        if self.upgrade.is_some() {
            PivotLevel::Upgraded
        } else {
            PivotLevel::Base
        }
    }

    /// 中枢覆盖的构件下标（首尾即绘制区间端点）。
    pub(crate) fn indices(&self) -> &[usize] {
        &self.indices
    }
}

/// 在原生贪心中枢之上优化前端与后端。
pub struct PivotAnalyzer<'a> {
    bis: &'a [BI],
}

impl<'a> PivotAnalyzer<'a> {
    pub fn new(bis: &'a [BI]) -> Self {
        Self { bis }
    }

    pub fn build(&self) -> Vec<ZS> {
        self.build_with_levels().0
    }

    /// 构造中枢及级别（与 [`Self::build`] 同源），级别与返回的中枢按顺序对齐。
    ///
    /// 级别为本级或九段升级后的高一级中枢，供 308/309 级别通道使用。
    pub(crate) fn build_with_levels(&self) -> (Vec<ZS>, Vec<PivotLevel>) {
        let units: Vec<PivotUnit> = self.bis.iter().map(PivotUnit::from_bi).collect();
        Self::build_selection(&units)
            .into_iter()
            .map(|selection| {
                let band = selection.band(&units);
                let level = selection.level();
                let mut zs = ZS::new(
                    selection
                        .indices()
                        .iter()
                        .map(|&i| self.bis[i].clone())
                        .collect(),
                );
                // 升级后区间不再等于前 3 笔重叠，需覆盖 `ZS::new` 的计算结果
                zs.zd = band.zd;
                zs.zg = band.zg;
                zs.zz = band.mid();
                (zs, level)
            })
            .unzip()
    }

    /// 对任意中枢构件做原生划分 + 前端/后端优化 + 九段升级，返回每个中枢的
    /// 构件下标与可选的高一级区间。
    ///
    /// 笔中枢与段中枢共用此实现：调用方只需提供 [`PivotUnit`] 视图，再把
    /// 选中的下标映射回自己的对象（`ZS` / `SegmentPivot`）。
    pub(crate) fn build_selection(units: &[PivotUnit]) -> Vec<PivotSelection> {
        let native = Self::native_groups(units);

        // 第一遍：前端优化 + 行情趋势链（方向按中枢高低位置递推）
        let mut fronts: Vec<(Vec<usize>, CzscDirection)> = Vec::new();
        let mut prev: Option<(f64, CzscDirection)> = None;
        for group in &native {
            let zz = PivotUnit::band(units, group).mid();
            // 行情方向：按中枢高低位置判定（当前中枢用原生中轴与前一中枢的
            // 最终中轴比较；剔除进入构件对中轴影响很小，不影响方向判定）
            let trend = match prev {
                None if zz > units[0].start_price => CzscDirection::Up,
                None => CzscDirection::Down,
                Some((prev_zz, _)) if zz > prev_zz => CzscDirection::Up,
                Some((prev_zz, _)) if zz < prev_zz => CzscDirection::Down,
                // 中轴平齐沿用前一趋势
                Some((_, previous)) => previous,
            };
            let Some(trimmed) = Self::trim_front(units, group, trend) else {
                continue;
            };
            prev = Some((PivotUnit::band(units, &trimmed).mid(), trend));
            fronts.push((trimmed, trend));
        }

        // 第二遍：后端优化（从后往前处理——需要后一保留中枢的起点判断相邻间隔，
        // 倒序时 result 的最后一个即"后一中枢"，被剔除的中枢自动跳过）
        let mut result: Vec<Vec<usize>> = Vec::new();
        for (group, trend) in fronts.into_iter().rev() {
            let next_start = result
                .last()
                .and_then(|next| next.first())
                .and_then(|&i| units[i].start_index);
            let Some(trimmed) = Self::trim_back(units, group, trend, next_start) else {
                continue;
            };
            result.push(trimmed);
        }
        result.reverse();
        Self::upgrade_chains(units, &result)
    }

    /// 九段升级：相邻且区间重叠的中枢串成一条链，链内构件跨度达到九段且
    /// 三三得九交集非空时合并为一个高一级中枢（替换子中枢），并继续吸收与
    /// 高一级区间重叠的后续中枢（被吸收者同样纳入包络）。升级区间由三三得九
    /// 交集与全部子中枢区间的包络决定，左右与上下都只加宽不收窄。只做一级
    /// 升级，升级结果不再参与二次合并。
    fn upgrade_chains(units: &[PivotUnit], groups: &[Vec<usize>]) -> Vec<PivotSelection> {
        let mut selections: Vec<PivotSelection> = Vec::new();
        let mut next = 0;
        while next < groups.len() {
            let Some(&first) = groups[next].first() else {
                next += 1;
                continue;
            };
            // 先按 level-1 区间重叠串链（扩展的候选范围）
            let mut end = next;
            while end + 1 < groups.len()
                && Self::groups_overlap(units, &groups[end], &groups[end + 1])
            {
                end += 1;
            }
            let Some(upgrade) = Self::chain_upgrade(units, &groups[next..=end]) else {
                selections.extend(groups[next..=end].iter().map(|group| PivotSelection {
                    indices: group.clone(),
                    upgrade: None,
                }));
                next = end + 1;
                continue;
            };
            // 高一级中枢必须包住全部子中枢：三三得九交集与子中枢包络取并，
            // 价格区间只加宽不收窄
            let mut band = groups[next..=end].iter().fold(upgrade, |band, group| {
                band.hull(PivotUnit::band(units, group))
            });
            // 升级后继续吸收与高一级区间重叠的后续中枢
            while end + 1 < groups.len() && PivotUnit::band(units, &groups[end + 1]).overlaps(band)
            {
                end += 1;
                band = band.hull(PivotUnit::band(units, &groups[end]));
            }
            // 组非空由 `native_groups` 保证；退化时以首个构件为端点，不丢中枢
            let last = groups[end].last().copied().unwrap_or(first);
            selections.push(PivotSelection {
                indices: (first..=last).collect(),
                upgrade: Some(band),
            });
            next = end + 1;
        }
        selections
    }

    /// 一条 level-1 中枢链升级后的区间：构建跨度不足九段或三三得九交集为空
    /// 时返回 `None`（保持原级别）。
    fn chain_upgrade(units: &[PivotUnit], chain: &[Vec<usize>]) -> Option<Band> {
        let first = *chain.first()?.first()?;
        let last = *chain.last()?.last()?;
        // 链内构件连续：中间被两端修剪掉的离开/连接构件计入九段跨度
        PivotUnit::upgrade_band(units, first, last)
    }

    /// 两个中枢组（构件下标）的区间是否相交。
    fn groups_overlap(units: &[PivotUnit], left: &[usize], right: &[usize]) -> bool {
        PivotUnit::band(units, left).overlaps(PivotUnit::band(units, right))
    }

    /// 原生贪心划分（与 `czsc-core::get_zs_seq` 一致）。
    fn native_groups(units: &[PivotUnit]) -> Vec<Vec<usize>> {
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (i, unit) in units.iter().enumerate() {
            // 构件完全离开当前中枢区间时另起一组；首构件无区间可直接开组。
            match groups.last_mut() {
                Some(current) if PivotUnit::stays_in(PivotUnit::band(units, current), unit) => {
                    current.push(i);
                }
                _ => groups.push(vec![i]),
            }
        }
        groups
    }

    /// 剔除中枢的进入/连接构件（前端），返回优化后的构件下标。
    fn trim_front(
        units: &[PivotUnit],
        group: &[usize],
        trend: CzscDirection,
    ) -> Option<Vec<usize>> {
        // 前端：首构件与趋势同向 = 进入/连接构件，剔除
        let skip = usize::from(units[*group.first()?].direction == trend);
        let indices = group
            .get(skip..)
            .filter(|rest| rest.len() >= PivotUnit::MIN_PIVOT_UNITS)?
            .to_vec();
        // 区间 [zd, zg] = 前 3 个构件的重叠（剔除进入构件后重新计算）
        PivotUnit::band(units, &indices)
            .is_valid()
            .then_some(indices)
    }

    /// 末构件是否必须剔除：与趋势同向（离开构件），或与后一中枢共享/相交端点。
    ///
    /// 间隔规则优先级低于前端：后中枢前端不动，由本中枢（前中枢）末端退回。
    fn leaves(
        units: &[PivotUnit],
        trend: CzscDirection,
        next_start: Option<usize>,
        index: usize,
    ) -> bool {
        let unit = &units[index];
        let shares = next_start.is_some_and(|start| unit.end_index.is_some_and(|end| end >= start));
        shares || unit.direction == trend
    }

    /// 剔除中枢的离开构件（后端），返回优化后的构件下标。
    fn trim_back(
        units: &[PivotUnit],
        group: Vec<usize>,
        trend: CzscDirection,
        next_start: Option<usize>,
    ) -> Option<Vec<usize>> {
        let mut indices = group;
        while indices.len() > PivotUnit::MIN_PIVOT_UNITS
            && indices
                .last()
                .is_some_and(|&i| Self::leaves(units, trend, next_start, i))
        {
            indices.pop();
        }
        let keep = indices.len() >= PivotUnit::MIN_PIVOT_UNITS
            && !indices
                .last()
                .is_some_and(|&i| Self::leaves(units, trend, next_start, i));
        keep.then_some(indices)
    }

    /// 三买/三卖缠论严格化：每个中枢最多一个三买（405）和一个三卖（406）。
    pub(crate) fn strictize_third_points(points: &mut Vec<BuySellPoint>, pivots: &[ZS]) {
        let mut seen: HashSet<(u32, usize)> = HashSet::new();
        points.retain(|p| {
            if p.func_id != 405 && p.func_id != 406 {
                return true;
            }
            // 归属到锚点之前最近结束的中枢（用中枢组最后一笔终点）
            let Some(zi) = pivots
                .iter()
                .rposition(|zs| zs.bis.last().end_index().is_some_and(|e| e < p.index))
            else {
                return false;
            };
            seen.insert((p.func_id, zi))
        });
    }
}

/// 段中枢：以已确认段为构件、由笔中枢同一套规则选出并计算几何。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SegmentPivot {
    band: Band,
    level: PivotLevel,
    start_index: Option<usize>,
    end_index: Option<usize>,
}

impl SegmentPivot {
    /// 中枢价格区间（上下沿与中轴由 [`Band`] 统一给出）。
    pub(crate) fn band(&self) -> Band {
        self.band
    }

    /// 中枢级别：本级或九段升级后的高一级中枢。
    pub(crate) fn level(&self) -> PivotLevel {
        self.level
    }

    /// 绘制区间：首个构件起点 → 末个构件终点（缺失时该中枢不绘制）。
    pub(crate) fn span(&self) -> Option<(usize, usize)> {
        let start = self.start_index?;
        let end = self.end_index?;
        Some((start.min(end), start.max(end)))
    }
}

/// 段中枢构建：只消费已确认段（`is_sure=true`），未确认段不参与。
pub(crate) struct SegmentPivotAnalyzer<'a> {
    bis: &'a [BI],
    segs: &'a [Segment],
}

impl<'a> SegmentPivotAnalyzer<'a> {
    pub(crate) fn new(bis: &'a [BI], segs: &'a [Segment]) -> Self {
        Self { bis, segs }
    }

    pub(crate) fn build(&self) -> Vec<SegmentPivot> {
        let units: Vec<PivotUnit> = self
            .segs
            .iter()
            .filter(|seg| seg.is_sure)
            .filter_map(|seg| {
                let start = self.bis.get(seg.start_bi_idx)?;
                let end = self.bis.get(seg.end_bi_idx)?;
                Some(PivotUnit {
                    direction: seg.direction,
                    high: start.get_high().max(end.get_high()),
                    low: start.get_low().min(end.get_low()),
                    start_index: start.start_index(),
                    end_index: end.end_index(),
                    start_price: start.fx_a.fx,
                })
            })
            .collect();

        PivotAnalyzer::build_selection(&units)
            .into_iter()
            .filter_map(|selection| {
                let indices = selection.indices();
                let first = *indices.first()?;
                let last = *indices.last()?;
                Some(SegmentPivot {
                    band: selection.band(&units),
                    level: selection.level(),
                    start_index: units[first].start_index,
                    end_index: units[last].end_index,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::fixtures::make_series_bi;
    use crate::analyzer::objects::BiAnchor;

    /// 构造构件视图（测试用）：绘制端点编码为 `index` / `index + 1`。
    fn unit(direction: CzscDirection, low: f64, high: f64, index: usize) -> PivotUnit {
        PivotUnit {
            direction,
            high,
            low,
            start_index: Some(index),
            end_index: Some(index + 1),
            start_price: low,
        }
    }

    /// 九段一三三得九：11 个交替构件经前端/后端修剪后剩 9 个，升级区间取
    /// 三组包络的交集，再与 level-1 子中枢区间取包络（只加宽不收窄）。
    #[test]
    fn upgrade_after_nine_units() {
        use CzscDirection::{Down, Up};
        let units = vec![
            unit(Up, 5.0, 20.0, 0), // 进入构件，前端剔除
            unit(Down, 12.0, 20.0, 1),
            unit(Up, 12.0, 18.0, 2),
            unit(Down, 13.0, 18.0, 3),
            unit(Up, 13.0, 19.0, 4),
            unit(Down, 14.0, 19.0, 5),
            unit(Up, 14.0, 18.0, 6),
            unit(Down, 15.0, 18.0, 7),
            unit(Up, 15.0, 19.0, 8),
            unit(Down, 16.0, 19.0, 9),
            unit(Up, 16.0, 18.0, 10), // 离开构件，后端剔除
        ];
        let selections = PivotAnalyzer::build_selection(&units);
        assert_eq!(selections.len(), 1, "九段中枢应升级为单个高一级中枢");
        assert_eq!(selections[0].indices(), &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        // 三三得九交集 (15,19)；子中枢 [13,18]；包络后 (13,19)
        assert_eq!(selections[0].band(&units), Band { zd: 13.0, zg: 19.0 });
    }

    /// 不足九段不升级：8 个构件仍按 level-1 处理。
    #[test]
    fn eight_units_stay_at_level_one() {
        use CzscDirection::{Down, Up};
        let units: Vec<PivotUnit> = (0..8)
            .map(|i| unit(if i % 2 == 0 { Up } else { Down }, 10.0, 20.0, i))
            .collect();
        assert!(PivotUnit::subtrends(&units, 0, 7).is_none());
        assert!(PivotUnit::upgrade_band(&units, 0, 7).is_none());
    }

    /// 尾巴不足 3 个构件时并入末段，不改变前 3 组的交集口径。
    #[test]
    fn tail_units_join_last_subtrend() {
        use CzscDirection::{Down, Up};
        let units: Vec<PivotUnit> = (0..11)
            .map(|i| unit(if i % 2 == 0 { Up } else { Down }, 10.0, 20.0, i))
            .collect();
        let trends = PivotUnit::subtrends(&units, 0, 10).expect("11 个构件应能三三得九");
        assert_eq!(trends.len(), 3, "尾巴应并入末段而不是新开一段");
        assert_eq!(
            PivotUnit::upgrade_band(&units, 0, 10),
            PivotUnit::upgrade_band(&units, 0, 8)
        );
    }

    /// 中枢扩展：相邻重叠中枢合计跨度九段时整条链升级。
    #[test]
    fn adjacent_overlapping_pivots_merge_at_nine() {
        use CzscDirection::{Down, Up};
        let units = vec![
            unit(Up, 10.0, 20.0, 0),
            unit(Down, 11.0, 20.0, 1),
            unit(Up, 11.0, 19.0, 2),
            // 连接构件：不属于任何中枢，但计入九段跨度
            unit(Down, 18.0, 26.0, 3),
            unit(Up, 18.0, 26.0, 4),
            unit(Down, 19.0, 26.0, 5),
            // 后一中枢：与前一中枢 [11,19] 重叠
            unit(Up, 12.0, 22.0, 6),
            unit(Down, 13.0, 22.0, 7),
            unit(Up, 13.0, 21.0, 8),
        ];
        let selections = PivotAnalyzer::upgrade_chains(&units, &[vec![0, 1, 2], vec![6, 7, 8]]);
        assert_eq!(selections.len(), 1, "九段链应合并为一个高一级中枢");
        assert_eq!(selections[0].indices(), &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
        // 三组包络 [10,20]、[18,26]、[12,22] 的交集 (18,20)；子中枢
        // [11,19] 与 [13,21]；包络后 (11,21)——高一级中枢不得窄于子中枢
        assert_eq!(selections[0].band(&units), Band { zd: 11.0, zg: 21.0 });
    }

    /// 相邻重叠中枢跨度不足九段时保持两个独立中枢。
    #[test]
    fn adjacent_overlapping_pivots_stay_below_nine() {
        use CzscDirection::{Down, Up};
        let units = vec![
            unit(Up, 10.0, 20.0, 0),
            unit(Down, 11.0, 20.0, 1),
            unit(Up, 11.0, 19.0, 2),
            unit(Down, 12.0, 19.0, 3),
            unit(Up, 12.0, 19.0, 4),
            unit(Down, 13.0, 21.0, 5),
            unit(Up, 13.0, 21.0, 6),
            unit(Down, 14.0, 21.0, 7),
        ];
        let selections = PivotAnalyzer::upgrade_chains(&units, &[vec![0, 1, 2], vec![5, 6, 7]]);
        assert_eq!(selections.len(), 2, "跨度 8 段不足九段");
        assert!(selections.iter().all(|s| s.upgrade.is_none()));
    }

    /// 升级不得缩窄：高一级区间必须包住链内每个子中枢的 `[zd, zg]`
    /// （三三得九交集比子中枢窄时以包络为准）。
    #[test]
    fn upgrade_never_narrows_child_pivots() {
        use CzscDirection::{Down, Up};
        let units = vec![
            unit(Up, 10.0, 20.0, 0),
            unit(Down, 11.0, 20.0, 1),
            unit(Up, 11.0, 19.0, 2),
            unit(Down, 16.0, 26.0, 3),
            unit(Up, 16.0, 26.0, 4),
            unit(Down, 17.0, 26.0, 5),
            unit(Up, 14.0, 24.0, 6),
            unit(Down, 15.0, 24.0, 7),
            unit(Up, 15.0, 23.0, 8),
        ];
        let children = [vec![0, 1, 2], vec![6, 7, 8]];
        let selections = PivotAnalyzer::upgrade_chains(&units, &children);
        assert_eq!(selections.len(), 1, "九段链应升级");
        let band = selections[0].band(&units);
        for child in &children {
            let child = PivotUnit::band(&units, child);
            assert!(
                band.zd <= child.zd && band.zg >= child.zg,
                "升级区间 [{},{}] 必须包住子中枢 [{},{}]",
                band.zd,
                band.zg,
                child.zd,
                child.zg
            );
        }
        // 三三得九交集 (16,20) 比子中枢窄，包络后应为 (11,23)
        assert_eq!(band, Band { zd: 11.0, zg: 23.0 });
    }

    /// 区间不重叠的中枢即使跨度达到九段也不合并（趋势而非扩展）。
    #[test]
    fn trend_pivots_never_merge() {
        use CzscDirection::{Down, Up};
        let units = vec![
            unit(Up, 10.0, 15.0, 0),
            unit(Down, 11.0, 15.0, 1),
            unit(Up, 11.0, 14.0, 2),
            unit(Down, 16.0, 20.0, 3),
            unit(Up, 16.0, 20.0, 4),
            unit(Down, 17.0, 20.0, 5),
            unit(Up, 30.0, 35.0, 6),
            unit(Down, 31.0, 35.0, 7),
            unit(Up, 31.0, 34.0, 8),
        ];
        let selections = PivotAnalyzer::upgrade_chains(&units, &[vec![0, 1, 2], vec![6, 7, 8]]);
        assert_eq!(selections.len(), 2, "区间不重叠的中枢不得合并");
    }

    /// 三组包络交集为空时安全降级，保持原级别。
    #[test]
    fn empty_upgrade_band_keeps_level_one() {
        use CzscDirection::{Down, Up};
        let units = vec![
            unit(Up, 10.0, 20.0, 0),
            unit(Down, 11.0, 20.0, 1),
            unit(Up, 11.0, 19.0, 2),
            unit(Down, 30.0, 40.0, 3),
            unit(Up, 30.0, 40.0, 4),
            unit(Down, 31.0, 40.0, 5),
            unit(Up, 12.0, 18.0, 6),
            unit(Down, 13.0, 18.0, 7),
            unit(Up, 13.0, 17.0, 8),
        ];
        let selections = PivotAnalyzer::upgrade_chains(&units, &[vec![0, 1, 2], vec![6, 7, 8]]);
        assert_eq!(selections.len(), 2, "无交集时退回原级别");
        assert!(selections.iter().all(|s| s.upgrade.is_none()));
    }

    /// 延伸：构件持续与区间相交时中枢不中断，完全离开的构件另起中枢。
    #[test]
    fn extension_continues_until_unit_leaves_band() {
        use CzscDirection::{Down, Up};
        let units = vec![
            unit(Up, 5.0, 20.0, 0), // 进入构件
            unit(Down, 12.0, 20.0, 1),
            unit(Up, 12.0, 18.0, 2),
            unit(Down, 13.0, 18.0, 3),
            unit(Up, 13.0, 19.0, 4), // 仍与区间相交，延伸进中枢后作为离开构件剔除
            unit(Down, 21.0, 26.0, 5), // 完全离开 [13,18] → 另起中枢
            unit(Up, 21.0, 26.0, 6),
            unit(Down, 22.0, 26.0, 7),
            unit(Up, 22.0, 25.0, 8),
        ];
        let selections = PivotAnalyzer::build_selection(&units);
        assert_eq!(selections.len(), 2, "完全离开构件应另起中枢");
        assert_eq!(selections[0].indices(), &[1, 2, 3]);
        assert_eq!(selections[1].indices(), &[5, 6, 7]);
    }

    /// 端到端：升级区间覆盖 `ZS::new` 的前 3 笔计算结果。
    #[test]
    fn upgrade_overrides_zs_band() {
        let prices = [
            5.0, 20.0, 12.0, 18.0, 13.0, 19.0, 14.0, 18.0, 15.0, 19.0, 16.0, 18.0,
        ];
        let bis: Vec<BI> = prices
            .windows(2)
            .enumerate()
            .map(|(i, pair)| make_series_bi(i as i64, pair[0], pair[1]))
            .collect();
        let pivots = PivotAnalyzer::new(&bis).build();
        assert_eq!(pivots.len(), 1, "九段中枢应替换子中枢");
        let zs = &pivots[0];
        assert_eq!(zs.bis.len(), 9);
        assert_eq!((zs.zd, zs.zg), (13.0, 19.0));
        assert_eq!(zs.zz, 16.0);
        assert_eq!(zs.bis.first().and_then(|b| b.start_index()), Some(1));
        assert_eq!(zs.bis.last().and_then(|b| b.end_index()), Some(10));
    }

    /// 中枢前端/后端优化（原生 get_zs_seq + build_pivots 全链路）：
    /// - 前端：上升行情中枢第一笔向下、下跌行情第一笔向上；
    /// - 后端：上升行情中枢最后一笔向下、下跌行情最后一笔向上（奇数笔
    ///   结构）；末笔与趋势同向（离开笔）被剔除；
    /// - 相邻间隔：前一个中枢后端与后一中枢前端至少间隔一笔（离开/连接
    ///   笔不属于任一中枢）；无法满足时整组剔除。
    ///
    /// 场景 1/2：上涨/下跌各两个中枢，均为 3 笔 [下,上,下]/[上,下,上]，
    /// 离开笔落在两中枢之间。场景 3：中枢向下破位（末笔跌破下沿）与后一
    /// 中枢共享端点，无法同时满足规则，整组剔除。场景 4：端点共享时前端
    /// 优先——前中枢先退一笔（间隔）、再退一笔（方向）。
    #[test]
    fn pivot_edges_follow_trend() {
        // 场景 1：上涨——两个中枢依次抬高，均为 [下,上,下]
        let up_pts = [
            (10.0, 20.0), // 0 上：窗口起点进入段（剔除）
            (20.0, 12.0), // 1 下
            (12.0, 19.0), // 2 上
            (19.0, 13.0), // 3 下
            (13.0, 25.0), // 4 上：离开笔（不属于任一中枢）
            (25.0, 22.0), // 5 下
            (22.0, 29.0), // 6 上
            (29.0, 23.0), // 7 下
            (23.0, 31.0), // 8 上：离开笔（不属于任一中枢）
            (31.0, 26.0), // 9 下：不成中枢
        ];
        let bis: Vec<BI> = up_pts
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| make_series_bi(i as i64, s, e))
            .collect();
        let pivots = PivotAnalyzer::new(&bis).build();
        assert_eq!(pivots.len(), 2, "上涨场景应保留两个中枢");
        for (k, zs) in pivots.iter().enumerate() {
            assert_eq!(zs.bis.len(), 3, "中枢{k}应为奇数笔 3 笔结构");
            assert_eq!(
                zs.sdir,
                CzscDirection::Down,
                "上涨行情中枢{k}第一笔必须向下"
            );
            assert_eq!(
                zs.bis.last().unwrap().direction,
                CzscDirection::Down,
                "上涨行情中枢{k}最后一笔必须向下"
            );
            if k > 0 {
                assert!(zs.zz > pivots[k - 1].zz, "后中枢必须高于前中枢");
            }
        }
        assert_eq!(pivots[0].bis[0].fx_a.fx, 20.0, "中枢 0 应从下跌笔 1 开始");
        assert_eq!(pivots[1].bis[0].fx_a.fx, 25.0, "中枢 1 应从下跌笔 5 开始");
        // 离开笔（13→25、23→31）不属于任一中枢
        for zs in &pivots {
            for b in &zs.bis {
                assert!(
                    b.fx_a.fx != 13.0 && b.fx_a.fx != 23.0,
                    "离开笔不得属于任一中枢"
                );
            }
        }
        // 相邻间隔：前中枢末笔终点下标 < 后中枢首笔起点下标（至少间隔一笔）
        assert!(
            pivots[0].bis.last().end_index().unwrap()
                < pivots[1].bis.first().start_index().unwrap(),
            "前中枢后端必须与后一中枢前端至少间隔一笔"
        );

        // 场景 2：下跌——两个中枢依次走低，均为 [上,下,上]
        let down_pts = [
            (20.0, 10.0), // 0 下：窗口起点进入段（剔除）
            (10.0, 18.0), // 1 上
            (18.0, 11.0), // 2 下
            (11.0, 17.0), // 3 上
            (17.0, 8.0),  // 4 下：离开笔（不属于任一中枢）
            (8.0, 10.0),  // 5 上
            (10.0, 6.0),  // 6 下
            (6.0, 9.0),   // 7 上
            (9.0, 5.0),   // 8 下：离开笔（不属于任一中枢）
            (5.0, 7.0),   // 9 上：不成中枢
        ];
        let bis: Vec<BI> = down_pts
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| make_series_bi(i as i64, s, e))
            .collect();
        let pivots = PivotAnalyzer::new(&bis).build();
        assert_eq!(pivots.len(), 2, "下跌场景应保留两个中枢");
        for (k, zs) in pivots.iter().enumerate() {
            assert_eq!(zs.bis.len(), 3, "中枢{k}应为奇数笔 3 笔结构");
            assert_eq!(zs.sdir, CzscDirection::Up, "下跌行情中枢{k}第一笔必须向上");
            assert_eq!(
                zs.bis.last().unwrap().direction,
                CzscDirection::Up,
                "下跌行情中枢{k}最后一笔必须向上"
            );
            if k > 0 {
                assert!(zs.zz < pivots[k - 1].zz, "后中枢必须低于前中枢");
            }
        }
        assert_eq!(pivots[0].bis[0].fx_a.fx, 10.0, "中枢 0 应从上涨笔 1 开始");
        assert_eq!(pivots[1].bis[0].fx_a.fx, 8.0, "中枢 1 应从上涨笔 5 开始");
        assert!(
            pivots[0].bis.last().end_index().unwrap()
                < pivots[1].bis.first().start_index().unwrap(),
            "前中枢后端必须与后一中枢前端至少间隔一笔"
        );

        // 场景 3：中枢 [12,19] 向下破位（末笔 19→8 跌破下沿），后一中枢起点
        // 与该末笔终点共享端点——3 笔无法再缩，整组剔除，仅保留后一中枢。
        let reversal_pts = [
            (10.0, 20.0), // 0 上：进入段（剔除）
            (20.0, 12.0), // 1 下
            (12.0, 19.0), // 2 上
            (19.0, 8.0),  // 3 下：破位，末笔终点与笔 4 起点共享
            (8.0, 10.0),  // 4 上：后一中枢首笔
            (10.0, 6.0),  // 5 下
            (6.0, 9.0),   // 6 上
            (9.0, 5.0),   // 7 下
            (5.0, 8.0),   // 8 上
        ];
        let bis: Vec<BI> = reversal_pts
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| make_series_bi(i as i64, s, e))
            .collect();
        let pivots = PivotAnalyzer::new(&bis).build();
        assert_eq!(pivots.len(), 1, "破位中枢无法满足间隔规则应整组剔除");
        assert_eq!(
            pivots[0].sdir,
            CzscDirection::Up,
            "下跌行情（后中枢更低）第一笔向上"
        );
        assert_eq!(pivots[0].bis[0].fx_a.fx, 8.0, "保留的中枢应从笔 4 开始");
        assert_eq!(pivots[0].bis.len(), 5, "后一中枢保留 5 笔（末笔向上）");

        // 场景 4：端点共享时前端优先——后中枢前端不动，前中枢末端先退一笔
        // （间隔），退回后末笔与趋势同向再退一笔（方向），剩 [下,上,下]。
        // 前中枢 [13,19] 末两笔为 U(13→25) 上破 + D(25→11) 深跌回踩（与
        // 后中枢首笔 U(11→11.5) 共享端点 11），先后退 D(25→11)（间隔）、
        // 再退 U(13→25)（方向）。
        let sweep_pts = [
            (10.0, 20.0), // 0 上：进入段（剔除）
            (20.0, 12.0), // 1 下
            (12.0, 19.0), // 2 上
            (19.0, 13.0), // 3 下
            (13.0, 25.0), // 4 上：上破
            (25.0, 11.0), // 5 下：深跌回踩，与笔 6 共享端点 11
            (11.0, 11.5), // 6 上：完全低于原生区间下沿（12）→ 新中枢首笔
            (11.5, 9.0),  // 7 下
            (9.0, 12.0),  // 8 上
            (12.0, 8.0),  // 9 下
            (8.0, 11.0),  // 10 上
        ];
        let bis: Vec<BI> = sweep_pts
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| make_series_bi(i as i64, s, e))
            .collect();
        let pivots = PivotAnalyzer::new(&bis).build();
        assert_eq!(pivots.len(), 2, "深跌回踩场景应保留两个中枢");
        // 前中枢：先退间隔一笔（笔 5），再退方向一笔（笔 4），剩 [下,上,下]
        assert_eq!(
            pivots[0].sdir,
            CzscDirection::Down,
            "前中枢（上升行情）第一笔向下"
        );
        assert_eq!(pivots[0].bis.len(), 3, "前中枢应退回两笔后剩 3 笔");
        assert_eq!(pivots[0].bis[0].fx_a.fx, 20.0, "前中枢应从下跌笔 1 开始");
        assert_eq!(
            pivots[0].bis.last().unwrap().fx_b.fx,
            13.0,
            "前中枢末笔应为 D(19→13)，不再包含笔 4/5"
        );
        // 后中枢：前端优先未被触碰（首笔 U 保留），趋势向下、末笔向上
        assert_eq!(
            pivots[1].sdir,
            CzscDirection::Up,
            "后中枢（下跌行情）第一笔向上"
        );
        assert_eq!(pivots[1].bis[0].fx_a.fx, 11.0, "后中枢首笔应为 U(11→11.5)");
        assert_eq!(pivots[1].bis.len(), 5, "后中枢保留 5 笔");
        assert_eq!(
            pivots[1].bis.last().unwrap().direction,
            CzscDirection::Up,
            "下跌行情后中枢末笔向上"
        );
        // 间隔：前中枢末笔终点（笔 3 终点）在笔 4/5 之后，与后中枢起点隔开
        assert!(
            pivots[0].bis.last().end_index().unwrap()
                < pivots[1].bis.first().start_index().unwrap(),
            "前中枢后端必须与后一中枢前端至少间隔一笔"
        );
    }
}
