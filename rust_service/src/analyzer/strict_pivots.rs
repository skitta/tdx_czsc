//! 严格派中枢：段中枢（level 1）→ 9 段升级 / 扩张（level 2）→ 扩张（level 3）。
//! 规则见 `docs/严格中枢规则.md`。

use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction as CzscDirection;

use super::objects::{bi_end_index, bi_start_index};
use super::segs::Segment;
use super::signals::BuySellPoint;
use crate::proto::analysis::SignalKind;

/// 段参与成枢的最小笔数（近似“完成次级别走势”）。
///
/// shortcut: 只用段笔数近似门槛，未逐个校验段内笔中枢；若实盘噪声大，升级为“段内含笔中枢”。
pub const MIN_SEG_BI_LEN: usize = 5;
/// 扩张合并的子中枢数上限（防止一路吞并）。
const MAX_EXPANSION_MEMBERS: usize = 4;
/// 扩张合并后允许的最大 K 线跨度。
const MAX_EXPANSION_SPAN_BARS: usize = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PivotKind {
    Segment,
    NineSegmentUpgrade,
    Expansion,
}

impl PivotKind {
    pub fn as_i32(self) -> i32 {
        match self {
            PivotKind::Segment => 1,
            PivotKind::NineSegmentUpgrade => 2,
            PivotKind::Expansion => 3,
        }
    }
}

/// 段（严格中枢的构成单元）。
#[derive(Debug, Clone, Copy)]
pub struct Move {
    pub seg_index: usize,
    pub high: f64,
    pub low: f64,
    pub start_index: usize,
    pub end_index: usize,
    pub is_sure: bool,
    pub up: bool,
    /// 是否满足成枢门槛（笔数足够且有可对齐的 K 线下标）。
    pub qualified: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct StrictPivot {
    pub level: u32,
    pub kind: PivotKind,
    pub is_sure: bool,
    pub start_index: usize,
    pub end_index: usize,
    pub upper: f64,
    pub lower: f64,
    pub middle: f64,
    pub gg: f64,
    pub dd: f64,
    pub move_count: usize,
    /// 构成段在 `segments` 中的起止下标（闭区间）。
    pub move_start: usize,
    pub move_end: usize,
}

#[derive(Debug, Clone, Default)]
pub struct StrictPivotSet {
    pub pivots: Vec<StrictPivot>,
    pub segments: Vec<Move>,
}

/// 段包络：段内全部构成笔的 `get_high()` 最大 / `get_low()` 最小。
pub fn seg_envelope(bis: &[BI], seg: &Segment) -> Option<(f64, f64)> {
    let slice = bis.get(seg.start_bi_idx..=seg.end_bi_idx)?;
    let mut high = f64::NEG_INFINITY;
    let mut low = f64::INFINITY;
    for bi in slice {
        high = high.max(bi.get_high());
        low = low.min(bi.get_low());
    }
    (high.is_finite() && low.is_finite()).then_some((high, low))
}

fn to_move(bis: &[BI], seg: &Segment, seg_index: usize) -> Move {
    let envelope = seg_envelope(bis, seg);
    let bi_count = seg.end_bi_idx - seg.start_bi_idx + 1;
    let (high, low) = envelope.unwrap_or((0.0, 0.0));
    Move {
        seg_index,
        high,
        low,
        start_index: bi_start_index(bis.get(seg.start_bi_idx)).unwrap_or(0),
        end_index: bi_end_index(bis.get(seg.end_bi_idx)).unwrap_or(0),
        is_sure: seg.is_sure,
        up: seg.direction == CzscDirection::Up,
        qualified: envelope.is_some() && bi_count >= MIN_SEG_BI_LEN,
    }
}

/// 构建全部级别的严格中枢（纯函数）。
pub fn build_strict_pivots(bis: &[BI], segs: &[Segment]) -> StrictPivotSet {
    let moves: Vec<Move> = segs
        .iter()
        .enumerate()
        .map(|(i, seg)| to_move(bis, seg, i))
        .collect();
    let level1 = segment_pivots(&moves);
    let (upgraded, upgraded_ids) = nine_segment_pivots(&moves, &level1);
    let expansion_members: Vec<StrictPivot> = level1
        .iter()
        .enumerate()
        .filter(|(i, _)| !upgraded_ids.contains(i))
        .map(|(_, p)| *p)
        .collect();
    let level2_expansion = expand_level(&expansion_members, 2);

    let mut pivots = level1.clone();
    pivots.extend(upgraded.iter().copied());
    pivots.extend(level2_expansion.iter().copied());
    let level2: Vec<StrictPivot> = upgraded
        .iter()
        .chain(level2_expansion.iter())
        .copied()
        .collect();
    pivots.extend(expand_level(&level2, 3));

    StrictPivotSet {
        pivots,
        segments: moves,
    }
}

/// 级别 1：3 段（已过门槛）重叠，后续有交集则延伸。
fn segment_pivots(moves: &[Move]) -> Vec<StrictPivot> {
    let mut pivots = Vec::new();
    let n = moves.len();
    let mut i = 0;
    while i + 2 < n {
        let (a, b, c) = (&moves[i], &moves[i + 1], &moves[i + 2]);
        if !(a.qualified && b.qualified && c.qualified) {
            i += 1;
            continue;
        }
        let upper = a.high.min(b.high).min(c.high);
        let lower = a.low.max(b.low).max(c.low);
        if upper < lower {
            i += 1;
            continue;
        }
        let mut pivot = StrictPivot {
            level: 1,
            kind: PivotKind::Segment,
            is_sure: a.is_sure && b.is_sure && c.is_sure,
            start_index: a.start_index,
            end_index: c.end_index,
            upper,
            lower,
            middle: (upper + lower) / 2.0,
            gg: a.high.max(b.high).max(c.high),
            dd: a.low.min(b.low).min(c.low),
            move_count: 3,
            move_start: i,
            move_end: i + 2,
        };
        let mut j = i + 3;
        while j < n {
            let m = &moves[j];
            let overlaps = m.qualified && m.high >= lower && m.low <= upper;
            if !overlaps {
                break;
            }
            pivot.gg = pivot.gg.max(m.high);
            pivot.dd = pivot.dd.min(m.low);
            pivot.is_sure &= m.is_sure;
            pivot.move_count += 1;
            pivot.move_end = j;
            pivot.end_index = m.end_index;
            j += 1;
        }
        pivots.push(pivot);
        // 下一轮从离开段之后扫描（离开段不属于任何中枢）。
        i = j + 1;
    }
    pivots
}

/// 3 段组的自身区间（`GZ >= GD` 才合法）。
fn group_range(moves: &[Move], start: usize) -> Option<(f64, f64)> {
    let a = moves.get(start)?;
    let b = moves.get(start + 1)?;
    let c = moves.get(start + 2)?;
    if !(a.qualified && b.qualified && c.qualified) {
        return None;
    }
    let gz = a.high.min(b.high).min(c.high);
    let gd = a.low.max(b.low).max(c.low);
    (gz >= gd).then_some((gz, gd))
}

/// 级别 2：9 段升级（3 个 3 段组；`move_count >= 9` 才尝试）。
fn nine_segment_pivots(moves: &[Move], level1: &[StrictPivot]) -> (Vec<StrictPivot>, Vec<usize>) {
    let mut out = Vec::new();
    let mut ids = Vec::new();
    for (idx, p) in level1.iter().enumerate() {
        if p.move_count < 9 {
            continue;
        }
        let start = p.move_start;
        let mut zg = 0.0;
        let mut zd = 0.0;
        let mut ok = start + 8 <= p.move_end;
        for g in 0..3 {
            let Some((gz, gd)) = group_range(moves, start + g * 3) else {
                ok = false;
                break;
            };
            if g == 0 {
                zg = gz;
                zd = gd;
            } else {
                zg = zg.min(gz);
                zd = zd.max(gd);
            }
            if zg < zd {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        let mut end = start + 8;
        let mut count = 3;
        let mut g = start + 9;
        while g + 2 <= p.move_end {
            let Some((gz, gd)) = group_range(moves, g) else {
                break;
            };
            if gz < zd || gd > zg {
                break;
            }
            zg = zg.min(gz);
            zd = zd.max(gd);
            if zg < zd {
                break;
            }
            end = g + 2;
            count += 1;
            g += 3;
        }
        let slice = &moves[start..=end];
        out.push(StrictPivot {
            level: 2,
            kind: PivotKind::NineSegmentUpgrade,
            is_sure: p.is_sure && slice.iter().all(|m| m.is_sure),
            start_index: p.start_index,
            end_index: moves[end].end_index,
            upper: zg,
            lower: zd,
            middle: (zg + zd) / 2.0,
            gg: slice
                .iter()
                .map(|m| m.high)
                .fold(f64::NEG_INFINITY, f64::max),
            dd: slice.iter().map(|m| m.low).fold(f64::INFINITY, f64::min),
            move_count: count,
            move_start: start,
            move_end: end,
        });
        ids.push(idx);
    }
    (out, ids)
}

/// 同级别中枢扩张：GG/DD 与 ZG/ZD 双条件 + 链长/跨度上限。
fn expand_level(members: &[StrictPivot], level: u32) -> Vec<StrictPivot> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < members.len() {
        let mut cur = members[i];
        let mut members_count = 1usize; // 已合并的子中枢数
        let mut j = i + 1;
        while j < members.len() {
            let next = members[j];
            let gg_overlap = cur.gg >= next.dd && next.gg >= cur.dd;
            let cores_overlap = cur.upper >= next.lower && next.upper >= cur.lower;
            if !gg_overlap || !cores_overlap {
                break;
            }
            if members_count + 1 > MAX_EXPANSION_MEMBERS
                || next.end_index.saturating_sub(cur.start_index) > MAX_EXPANSION_SPAN_BARS
            {
                break;
            }
            let upper = cur.gg.min(next.gg);
            let lower = cur.dd.max(next.dd);
            if upper < lower {
                break;
            }
            cur.upper = upper;
            cur.lower = lower;
            cur.middle = (upper + lower) / 2.0;
            cur.gg = cur.gg.max(next.gg);
            cur.dd = cur.dd.min(next.dd);
            cur.end_index = next.end_index;
            cur.is_sure &= next.is_sure;
            cur.level = level;
            cur.kind = PivotKind::Expansion;
            members_count += 1;
            cur.move_count = members_count;
            j += 1;
        }
        if members_count >= 2 {
            out.push(cur);
        }
        i = j;
    }
    out
}

/// 每根 bar 至多一个可见中枢：实优先、级别高优先、move_count 大优先、开始靠后优先。
pub fn select_visible(pivots: &[StrictPivot], data_len: usize) -> Vec<StrictPivot> {
    let mut order: Vec<usize> = (0..pivots.len()).collect();
    order.sort_by(|&a, &b| priority(&pivots[b]).cmp(&priority(&pivots[a])));
    let mut occupied = vec![false; data_len.max(1)];
    let mut visible = Vec::new();
    for idx in order {
        let p = &pivots[idx];
        if p.end_index >= occupied.len() || p.start_index > p.end_index {
            continue;
        }
        if occupied[p.start_index..=p.end_index].iter().any(|x| *x) {
            continue;
        }
        for x in &mut occupied[p.start_index..=p.end_index] {
            *x = true;
        }
        visible.push(*p);
    }
    visible.sort_by_key(|p| p.start_index);
    visible
}

fn priority(p: &StrictPivot) -> (u8, u32, u32, u32) {
    (
        p.is_sure as u8,
        p.level,
        p.move_count as u32,
        p.start_index as u32,
    )
}

/// 段级三买/三卖（407/408）：只对已确认的级别 1 中枢。
pub fn build_strict_signals(set: &StrictPivotSet) -> Vec<BuySellPoint> {
    let mut points = Vec::new();
    for p in set
        .pivots
        .iter()
        .filter(|p| p.kind == PivotKind::Segment && p.level == 1 && p.is_sure)
    {
        let (Some(leave), Some(confirm)) = (
            set.segments.get(p.move_end + 1),
            set.segments.get(p.move_end + 2),
        ) else {
            continue;
        };
        if !leave.is_sure || !confirm.is_sure || !leave.qualified {
            continue;
        }
        if !leave.up && leave.low > p.upper && confirm.low > p.upper {
            points.push(BuySellPoint {
                func_id: 407,
                kind: SignalKind::SignalSegBuy3,
                index: leave.end_index,
                price: leave.low,
                label: "段级三买".to_string(),
            });
        } else if leave.up && leave.high < p.lower && confirm.high < p.lower {
            points.push(BuySellPoint {
                func_id: 408,
                kind: SignalKind::SignalSegSell3,
                index: leave.end_index,
                price: leave.high,
                label: "段级三卖".to_string(),
            });
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::fixtures::make_series_bi;

    /// 用价格拐点构造连续笔：笔 k = pts[k] -> pts[k+1]。
    fn bis_from(pts: &[f64]) -> Vec<BI> {
        pts.windows(2)
            .enumerate()
            .map(|(i, w)| make_series_bi(i as i64, w[0], w[1]))
            .collect()
    }

    fn seg(start: usize, end: usize, is_sure: bool) -> Segment {
        Segment {
            start_bi_idx: start,
            end_bi_idx: end,
            direction: CzscDirection::Up,
            is_sure,
        }
    }

    #[test]
    fn three_qualified_segments_form_pivot_with_fixed_range() {
        let pts = [
            15.0, 20.0, 13.0, 18.0, 14.0, 16.0, 17.0, 13.0, 17.0, 14.0, 16.0, 15.0, 12.0, 16.0,
            13.0, 15.0,
        ];
        let bis = bis_from(&pts);
        let segs = [seg(0, 4, true), seg(5, 9, true), seg(10, 14, true)];
        let set = build_strict_pivots(&bis, &segs);
        let p = set
            .pivots
            .iter()
            .find(|p| p.level == 1)
            .expect("level1 pivot");
        assert_eq!((p.upper, p.lower), (16.0, 13.0));
        assert_eq!(p.move_count, 3);
        assert!(p.is_sure);
        assert_eq!(p.kind, PivotKind::Segment);
    }

    #[test]
    fn unqualified_short_segments_do_not_form_pivot() {
        let pts = [10.0, 20.0, 11.0, 19.0, 12.0, 18.0, 13.0, 17.0, 14.0, 16.0];
        let bis = bis_from(&pts);
        let segs = [seg(0, 2, true), seg(3, 5, true), seg(6, 8, true)];
        assert!(build_strict_pivots(&bis, &segs).pivots.is_empty());
    }

    #[test]
    fn leaving_segment_ends_pivot_and_is_excluded() {
        let pts = [
            15.0, 20.0, 13.0, 18.0, 14.0, 16.0, 17.0, 13.0, 17.0, 14.0, 16.0, 15.0, 12.0, 16.0,
            13.0, 15.0, 20.0, 17.0, 19.0, 18.0, 18.0, 22.0, 19.0, 21.0, 20.0, 21.0,
        ];
        let bis = bis_from(&pts);
        let segs = [
            seg(0, 4, true),
            seg(5, 9, true),
            seg(10, 14, true),
            seg(15, 19, true),
            seg(20, 24, true),
        ];
        let set = build_strict_pivots(&bis, &segs);
        let p = set
            .pivots
            .iter()
            .find(|p| p.level == 1)
            .expect("level1 pivot");
        assert_eq!(p.move_count, 4, "跨界段并入、整体在外段不得并入");
        assert_eq!(p.move_end, 3);
        assert_eq!(p.end_index, set.segments[3].end_index);
    }

    #[test]
    fn virtual_pivot_when_any_segment_unsure() {
        let pts = [
            15.0, 20.0, 13.0, 18.0, 14.0, 16.0, 17.0, 13.0, 17.0, 14.0, 16.0, 15.0, 12.0, 16.0,
            13.0, 15.0,
        ];
        let bis = bis_from(&pts);
        let segs = [seg(0, 4, false), seg(5, 9, true), seg(10, 14, true)];
        let set = build_strict_pivots(&bis, &segs);
        let p = set.pivots.first().expect("pivot");
        assert!(!p.is_sure);
    }

    /// 9 段全部落在同一区间 → 升级为 level 2（3 个 3 段组）。
    #[test]
    fn nine_segments_upgrade_to_level2() {
        let mut pts = vec![10.0, 20.0];
        for _ in 0..44 {
            pts.push(if pts.len() % 2 == 0 { 20.0 } else { 10.0 });
        }
        let bis = bis_from(&pts);
        let segs: Vec<Segment> = (0..9).map(|k| seg(k * 5, k * 5 + 4, true)).collect();
        let set = build_strict_pivots(&bis, &segs);
        let up = set
            .pivots
            .iter()
            .find(|p| p.kind == PivotKind::NineSegmentUpgrade)
            .expect("9 段升级");
        assert_eq!(up.level, 2);
        assert_eq!(up.move_count, 3);
        assert_eq!((up.upper, up.lower), (20.0, 10.0));
    }

    /// 扩张：GG/DD 重叠但核心区间不重叠 → 不合并；双条件满足 → level 2。
    #[test]
    fn expansion_requires_core_overlap() {
        let mk = |upper: f64, lower: f64, gg: f64, dd: f64, start: usize, end: usize| StrictPivot {
            level: 1,
            kind: PivotKind::Segment,
            is_sure: true,
            start_index: start,
            end_index: end,
            upper,
            lower,
            middle: (upper + lower) / 2.0,
            gg,
            dd,
            move_count: 3,
            move_start: 0,
            move_end: 2,
        };
        let far = [
            mk(20.0, 18.0, 100.0, 5.0, 0, 10),
            mk(90.0, 88.0, 95.0, 85.0, 12, 20),
        ];
        assert!(expand_level(&far, 2).is_empty(), "核心区不重叠不得扩张");

        let near = [
            mk(20.0, 18.0, 22.0, 15.0, 0, 10),
            mk(19.0, 15.0, 21.0, 14.0, 12, 20),
        ];
        let merged = expand_level(&near, 2);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].kind, PivotKind::Expansion);
        assert_eq!(merged[0].level, 2);
        assert_eq!(merged[0].move_count, 2);
    }

    /// 可见性：实中枢优先于高级别虚中枢。
    #[test]
    fn visible_prefers_sure_pivot() {
        let mk = |level: u32, sure: bool, start: usize, end: usize| StrictPivot {
            level,
            kind: PivotKind::Segment,
            is_sure: sure,
            start_index: start,
            end_index: end,
            upper: 10.0,
            lower: 8.0,
            middle: 9.0,
            gg: 11.0,
            dd: 7.0,
            move_count: 3,
            move_start: 0,
            move_end: 2,
        };
        let pivots = [mk(1, true, 0, 5), mk(2, false, 2, 8)];
        let visible = select_visible(&pivots, 20);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].level, 1, "已确认的低级别优先于虚高级别");
    }

    /// 三买锚在离开段终点，而非随后上涨段顶部。
    #[test]
    fn third_buy_anchors_at_leaving_segment_end() {
        let pts = [
            15.0, 20.0, 13.0, 18.0, 14.0, 16.0, // seg1
            17.0, 13.0, 17.0, 14.0, 16.0, // seg2
            15.0, 12.0, 16.0, 13.0, 15.0, // seg3 → 中枢 [13,16]
            20.0, 17.0, 19.0, 18.0, 18.0, // seg4 上破（延伸）
            19.0, 17.0, 18.0, 17.0, 17.0, // seg5 下跌离开（low 17 > 16）
            17.0, 19.0, 18.0, 20.0, 19.0, // seg6 向上确认
        ];
        let bis = bis_from(&pts);
        let mut segs = vec![
            seg(0, 4, true),
            seg(5, 9, true),
            seg(10, 14, true),
            seg(15, 19, true),
        ];
        segs.push(Segment {
            start_bi_idx: 20,
            end_bi_idx: 24,
            direction: CzscDirection::Down,
            is_sure: true,
        });
        segs.push(seg(25, 29, true));
        let set = build_strict_pivots(&bis, &segs);
        let points = build_strict_signals(&set);
        let buy = points.iter().find(|p| p.func_id == 407).expect("三买");
        assert_eq!(buy.index, set.segments[4].end_index);
        assert!((buy.price - 17.0).abs() < 1e-9);
    }
}
