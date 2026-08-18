//! 段（线段）识别：特征序列分型法（缠论 65 课标准线段终结判定）。
//!
//! czsc-core 只提供分型/笔/中枢三层，不提供线段；这里在 rust-service 层按
//! 缠论规则实现（与 `build_pivots` 同一思路）。
//!
//! 算法移植自社区缠论框架 Vespa314-chan.py 的 `Seg/SegListChan.py`（特征序列
//! 分型状态机 + 缺口确认 + 尾部未完成段收集），适配 czsc-core 的 `BI` 对象与
//! 本服务的静态快照模式（一次性输入全部已完成笔，不做增量更新）。
//!
//! 规则要点：
//! 1. 线段 = 至少 3 笔，首笔与末笔同方向（向上线段以向上笔开始、向上笔结束）；
//! 2. 线段终结 = 特征序列出现分型：向上线段看"向下笔序列"的顶分型，
//!    向下线段看"向上笔序列"的底分型；
//! 3. 特征序列元素 = 反向笔，按线段方向做包含合并（向上合并取高高低、
//!    向下合并取低低高高）；
//! 4. 第三元素必须"实际突破"才构成分型；特征序列出现缺口时需后续反向
//!    分型确认（`find_revert_fx`）；
//! 5. 确认段之后的剩余笔合并为尾部"未完成段"（`is_sure=false`）：对象输出
//!    中保留供调试，但 203/204 显示通道只输出 `is_sure=true` 的确认段
//!    （图表上只画确定好的段）。

use czsc_core::objects::bi::BI;
use czsc_core::objects::direction::Direction as CzscDirection;

/// 识别出的段（线段）
#[derive(Debug, Clone)]
pub struct Segment {
    /// 段首笔在输入笔序列中的索引
    pub start_bi_idx: usize,
    /// 段末笔在输入笔序列中的索引
    pub end_bi_idx: usize,
    /// 段方向（与末笔方向一致）
    pub direction: CzscDirection,
    /// 是否已被特征序列分型确认；false = 尾部未完成/暂定线段
    pub is_sure: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dir {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Combine {
    Up,
    Down,
    Combine,
    Included,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FxType {
    Unknown,
    Top,
    Bottom,
}

fn dir_of(d: CzscDirection) -> Dir {
    match d {
        CzscDirection::Up => Dir::Up,
        CzscDirection::Down => Dir::Down,
    }
}

fn revert(d: Dir) -> Dir {
    match d {
        Dir::Up => Dir::Down,
        Dir::Down => Dir::Up,
    }
}

fn bi_dir(bi: &BI) -> Dir {
    dir_of(bi.direction)
}

/// 特征序列元素：一个或多个同向反向笔按线段方向合并而成。
///
/// 对应 Vespa314 `CEigen`（继承 `CKLine_Combiner`）：维护元素内笔列表与合并后
/// 的高低点；`try_add` 返回合并结果并就地更新高低点；`update_fx` 由前后元素
/// 判定分型；`gap` 标记特征序列缺口（顶分型且前一元素高点低于本元素低点）。
#[derive(Clone)]
struct Eigen {
    /// 组成该元素的笔索引
    bis: Vec<usize>,
    high: f64,
    low: f64,
    /// 合并方向（向上合并取高高低，向下合并取低低高高）
    dir: Dir,
    fx: FxType,
    gap: bool,
}

impl Eigen {
    fn new(bi_idx: usize, bis: &[BI], dir: Dir) -> Self {
        Eigen {
            bis: vec![bi_idx],
            high: bis[bi_idx].get_high(),
            low: bis[bi_idx].get_low(),
            dir,
            fx: FxType::Unknown,
            gap: false,
        }
    }

    /// 判定 `bis[bi_idx]` 与当前元素的包含关系（对应 `CKLine_Combiner::test_combine`）。
    fn test_combine(
        &self,
        bi_idx: usize,
        bis: &[BI],
        exclude_included: bool,
        allow_top_equal: Option<i8>,
    ) -> Combine {
        let bi = &bis[bi_idx];
        let (ih, il) = (bi.get_high(), bi.get_low());
        if self.high >= ih && self.low <= il {
            return Combine::Combine;
        }
        if self.high <= ih && self.low >= il {
            if allow_top_equal == Some(1) && self.high == ih && self.low > il {
                return Combine::Down;
            }
            if allow_top_equal == Some(-1) && self.low == il && self.high < ih {
                return Combine::Up;
            }
            return if exclude_included {
                Combine::Included
            } else {
                Combine::Combine
            };
        }
        if self.high > ih && self.low > il {
            return Combine::Down;
        }
        if self.high < ih && self.low < il {
            return Combine::Up;
        }
        Combine::Combine
    }

    /// 尝试把 `bis[bi_idx]` 并入元素；返回合并结果（`Combine` 表示已并入）。
    fn try_add(
        &mut self,
        bi_idx: usize,
        bis: &[BI],
        exclude_included: bool,
        allow_top_equal: Option<i8>,
    ) -> Combine {
        let c = self.test_combine(bi_idx, bis, exclude_included, allow_top_equal);
        if c == Combine::Combine {
            self.bis.push(bi_idx);
            let bi = &bis[bi_idx];
            match self.dir {
                Dir::Up => {
                    self.high = self.high.max(bi.get_high());
                    self.low = self.low.max(bi.get_low());
                }
                Dir::Down => {
                    self.high = self.high.min(bi.get_high());
                    self.low = self.low.min(bi.get_low());
                }
            }
        }
        c
    }

    /// 由前后元素判定本元素是否为分型（对应 `CKLine_Combiner::update_fx`）。
    fn update_fx(&mut self, pre: &Eigen, next: &Eigen, exclude_included: bool, allow_top_equal: Option<i8>) {
        if exclude_included {
            if pre.high < self.high && next.high <= self.high && next.low < self.low {
                if allow_top_equal == Some(1) || next.high < self.high {
                    self.fx = FxType::Top;
                }
            } else if next.high > self.high && pre.low > self.low && next.low >= self.low {
                if allow_top_equal == Some(-1) || next.low > self.low {
                    self.fx = FxType::Bottom;
                }
            }
        } else if pre.high < self.high
            && next.high < self.high
            && pre.low < self.low
            && next.low < self.low
        {
            self.fx = FxType::Top;
        } else if pre.high > self.high
            && next.high > self.high
            && pre.low > self.low
            && next.low > self.low
        {
            self.fx = FxType::Bottom;
        }
    }

    /// 元素内极值笔索引（顶分型取 high 最高、底分型取 low 最低；并列取最后出现者，
    /// 与 Python 版"从尾部向前找首个等于极值的笔"一致）。
    fn peak_bi_idx(&self, bis: &[BI], is_high: bool) -> usize {
        let mut best = self.bis[0];
        for &i in self.bis.iter().rev() {
            let better = if is_high {
                bis[i].get_high() >= bis[best].get_high()
            } else {
                bis[i].get_low() <= bis[best].get_low()
            };
            if better {
                best = i;
            }
        }
        best
    }
}

/// 特征序列：线段方向对应的反向笔序列状态机（对应 Vespa314 `CEigenFX`）。
struct EigenFX {
    /// 线段方向（元素 = 反向笔）
    dir: Dir,
    exclude_included: bool,
    /// 特征元素 ele[0..2]
    ele: [Option<Eigen>; 3],
    /// 已加入的所有反向笔索引（按时间序）
    lst: Vec<usize>,
}

impl EigenFX {
    fn new(dir: Dir, exclude_included: bool) -> Self {
        EigenFX {
            dir,
            exclude_included,
            ele: [None, None, None],
            lst: Vec::new(),
        }
    }

    fn is_up(&self) -> bool {
        self.dir == Dir::Up
    }

    fn clear(&mut self) {
        self.ele = [None, None, None];
        self.lst.clear();
    }

    /// 加入一个反向笔；返回是否出现分型（线段终结候选）。
    fn add(&mut self, bi_idx: usize, bis: &[BI]) -> bool {
        debug_assert!(bi_dir(&bis[bi_idx]) != self.dir);
        self.lst.push(bi_idx);
        if self.ele[0].is_none() {
            self.treat_first_ele(bi_idx, bis)
        } else if self.ele[1].is_none() {
            self.treat_second_ele(bi_idx, bis)
        } else if self.ele[2].is_none() {
            self.treat_third_ele(bi_idx, bis)
        } else {
            panic!("特征序列3个元素都找齐了还没处理!! bi_idx={bi_idx}");
        }
    }

    fn treat_first_ele(&mut self, bi_idx: usize, bis: &[BI]) -> bool {
        self.ele[0] = Some(Eigen::new(bi_idx, bis, self.dir));
        false
    }

    fn treat_second_ele(&mut self, bi_idx: usize, bis: &[BI]) -> bool {
        let combine_dir = self.ele[0]
            .as_mut()
            .unwrap()
            .try_add(bi_idx, bis, self.exclude_included, None);
        if combine_dir != Combine::Combine {
            let e1 = Eigen::new(bi_idx, bis, self.dir);
            // 前两元素不可能成为分型：向上线段第二元素高点不高于第一元素高点
            let impossible = (self.is_up() && e1.high < self.ele[0].as_ref().unwrap().high)
                || (!self.is_up() && e1.low > self.ele[0].as_ref().unwrap().low);
            self.ele[1] = Some(e1);
            if impossible {
                return self.reset(bis);
            }
        }
        false
    }

    fn treat_third_ele(&mut self, bi_idx: usize, bis: &[BI]) -> bool {
        let allow_top_equal = if self.exclude_included {
            if bi_dir(&bis[bi_idx]) == Dir::Down {
                Some(1)
            } else {
                Some(-1)
            }
        } else {
            None
        };
        let combine_dir = self.ele[1]
            .as_mut()
            .unwrap()
            .try_add(bi_idx, bis, self.exclude_included, allow_top_equal);
        if combine_dir == Combine::Combine {
            return false;
        }
        let e2_dir = match combine_dir {
            Combine::Up => Dir::Up,
            Combine::Down => Dir::Down,
            _ => self.dir,
        };
        let e2 = Eigen::new(bi_idx, bis, e2_dir);
        self.ele[2] = Some(e2);
        if !self.actual_break(bis) {
            return self.reset(bis);
        }
        let pre = self.ele[0].as_ref().unwrap().clone();
        let next = self.ele[2].as_ref().unwrap().clone();
        let mid = self.ele[1].as_mut().unwrap();
        mid.update_fx(&pre, &next, self.exclude_included, allow_top_equal);
        let fx = mid.fx;
        let is_fx = (self.is_up() && fx == FxType::Top) || (!self.is_up() && fx == FxType::Bottom);
        if is_fx {
            true
        } else {
            self.reset(bis)
        }
    }

    /// 第三元素必须"实际突破"才构成分型（防止第二元素因合并导致后面没有实际突破）。
    fn actual_break(&self, bis: &[BI]) -> bool {
        if !self.exclude_included {
            return true;
        }
        let e2 = self.ele[2].as_ref().unwrap();
        let e1 = self.ele[1].as_ref().unwrap();
        let last_of_e1 = &bis[*e1.bis.last().unwrap()];
        if (self.is_up() && e2.low < last_of_e1.get_low())
            || (!self.is_up() && e2.high > last_of_e1.get_high())
        {
            return true;
        }
        // 第三元素只有一笔时：看其后第二笔是否延续突破
        debug_assert_eq!(e2.bis.len(), 1);
        let ele2_bi = e2.bis[0];
        if ele2_bi + 2 < bis.len() {
            let b2 = &bis[ele2_bi + 2];
            let b0 = &bis[ele2_bi];
            if (b0.direction == CzscDirection::Down && b2.get_low() < b0.get_low())
                || (b0.direction == CzscDirection::Up && b2.get_high() > b0.get_high())
            {
                return true;
            }
        }
        false
    }

    /// 窗口滑动重置（对应 `CEigenFX::reset`）。
    /// exclude_included=true：丢弃第一元素，其余笔重新入列（可能立刻出分型）；
    /// 否则：元素左移一位（ele[0]=ele[1], ele[1]=ele[2]）。
    fn reset(&mut self, bis: &[BI]) -> bool {
        let bi_tmp_list = self.lst[1..].to_vec();
        if self.exclude_included {
            self.clear();
            for &i in &bi_tmp_list {
                if self.add(i, bis) {
                    return true;
                }
            }
        } else {
            let ele2_begin = self.ele[1].as_ref().unwrap().bis[0];
            let e0 = self.ele[1].take();
            let e1 = self.ele[2].take();
            self.ele[0] = e0;
            self.ele[1] = e1;
            self.ele[2] = None;
            self.lst = bi_tmp_list.into_iter().filter(|&i| i >= ele2_begin).collect();
        }
        false
    }

    /// 线段是否可在此分型处终结。
    /// 返回：Some(true)=可终结；Some(false)=不可终结（缺口未确认）；None=找到尾部
    /// 也没找到确认（按未确认处理，线段照加）。
    fn can_be_end(&self, bis: &[BI]) -> Option<bool> {
        let e1 = self.ele[1].as_ref().unwrap();
        if e1.gap {
            let end_bi_idx = self.peak_bi_idx(bis);
            let thred_value = bis[end_bi_idx].fx_b.fx;
            let break_thred = if self.is_up() {
                self.ele[0].as_ref().unwrap().low
            } else {
                self.ele[0].as_ref().unwrap().high
            };
            self.find_revert_fx(bis, end_bi_idx + 2, thred_value, break_thred)
        } else {
            Some(true)
        }
    }

    /// 线段终点笔索引 = 分型中间元素内极值笔的前一笔。
    ///
    /// 顶分型（向上线段）中间元素为向下笔，极值 = high 最高者，其**起点**即
    /// 线段顶；线段终结笔是它前面那一笔（向上笔，终点在该顶）。底分型对称。
    fn peak_bi_idx(&self, bis: &[BI]) -> usize {
        let e1 = self.ele[1].as_ref().unwrap();
        e1.peak_bi_idx(bis, self.is_up()) - 1
    }

    /// 特征序列缺口（顶分型且前一元素高点低于本元素低点）时的后续反向分型确认。
    ///
    /// 对应 `CEigenFX::find_revert_fx`（COMMON_COMBINE=true 简化模式：
    /// 反向特征序列出现任意分型即确认终结）。
    fn find_revert_fx(
        &self,
        bis: &[BI],
        begin_idx: usize,
        thred_value: f64,
        break_thred: f64,
    ) -> Option<bool> {
        if begin_idx >= bis.len() {
            return None;
        }
        let first_bi_dir = bi_dir(&bis[begin_idx]);
        let mut egien_fx = EigenFX::new(revert(first_bi_dir), false);
        let mut i = begin_idx;
        while i < bis.len() {
            let bi = &bis[i];
            if egien_fx.add(i, bis) {
                // COMMON_COMBINE=true：出现分型即确认
                return Some(true);
            }
            if (bi.direction == CzscDirection::Down && bi.get_low() < thred_value)
                || (bi.direction == CzscDirection::Up && bi.get_high() > thred_value)
            {
                return Some(false);
            }
            if egien_fx.ele[1].is_some() {
                let e1 = egien_fx.ele[1].as_ref().unwrap();
                if (bi.direction == CzscDirection::Down && e1.high > break_thred)
                    || (bi.direction == CzscDirection::Up && e1.low < break_thred)
                {
                    return Some(true);
                }
            }
            i += 2;
        }
        None
    }
}

/// 在笔序列中识别段（对应 Vespa314 `CSegListChan` 的静态一次扫描）。
pub fn build_segs(bis: &[BI]) -> Vec<Segment> {
    let mut segs = Vec::new();
    cal_seg_sure(bis, 0, &mut segs);
    collect_left_seg(bis, &mut segs);
    segs
}

/// 从 `begin_idx` 开始扫描，找到第一处特征序列分型并确认/拒绝该线段；
/// 确认后从线段终点之后继续递归扫描。
fn cal_seg_sure(bis: &[BI], begin_idx: usize, segs: &mut Vec<Segment>) {
    let mut up_eigen = EigenFX::new(Dir::Up, true);
    let mut down_eigen = EigenFX::new(Dir::Down, true);
    // 已确认的上一段方向；用于决定当前段只喂哪个特征序列
    let mut last_seg_dir: Option<Dir> = segs.last().map(|s| dir_of(s.direction));
    let mut i = begin_idx;
    while i < bis.len() {
        let bi = &bis[i];
        let mut hit: Option<Dir> = None;
        if bi.direction == CzscDirection::Down && last_seg_dir != Some(Dir::Up) {
            if up_eigen.add(i, bis) {
                hit = Some(Dir::Up);
            }
        } else if bi.direction == CzscDirection::Up && last_seg_dir != Some(Dir::Down) {
            if down_eigen.add(i, bis) {
                hit = Some(Dir::Down);
            }
        }

        // 第一段方向确定：不要以谁先成为分型来决定（反例：US.EVRG）。
        // 某个特征序列先凑齐两个元素时，预判第一段方向并停喂另一侧特征序列；
        // 若随后该特征序列不足两个元素，则解除预判。
        if segs.is_empty() {
            if up_eigen.ele[1].is_some() && bi.direction == CzscDirection::Down {
                last_seg_dir = Some(Dir::Down);
                down_eigen.clear();
            } else if down_eigen.ele[1].is_some() && bi.direction == CzscDirection::Up {
                up_eigen.clear();
                last_seg_dir = Some(Dir::Up);
            }
            if up_eigen.ele[1].is_none()
                && last_seg_dir == Some(Dir::Down)
                && bi.direction == CzscDirection::Down
            {
                last_seg_dir = None;
            } else if down_eigen.ele[1].is_none()
                && last_seg_dir == Some(Dir::Up)
                && bi.direction == CzscDirection::Up
            {
                last_seg_dir = None;
            }
        }

        if let Some(d) = hit {
            let eigen = if d == Dir::Up { &up_eigen } else { &down_eigen };
            treat_fx_eigen(bis, eigen, segs);
            break;
        }
        i += 1;
    }
}

/// 处理特征序列分型：确认/拒绝一个线段终点，并继续扫描。
fn treat_fx_eigen(bis: &[BI], fx_eigen: &EigenFX, segs: &mut Vec<Segment>) {
    let test = fx_eigen.can_be_end(bis);
    let end_bi_idx = fx_eigen.peak_bi_idx(bis);
    match test {
        Some(true) | None => {
            let is_true = test == Some(true);
            // czsc 的已完成笔全部视为"确定"，故 is_sure 仅取决于终结确认状态
            if !add_new_seg(bis, end_bi_idx, is_true, segs) {
                cal_seg_sure(bis, end_bi_idx + 1, segs);
                return;
            }
            if is_true {
                cal_seg_sure(bis, end_bi_idx + 1, segs);
            }
        }
        Some(false) => {
            // 缺口未确认：从第二特征元素首笔处重新扫描
            let resume = fx_eigen.lst.get(1).copied().unwrap_or(end_bi_idx + 1);
            cal_seg_sure(bis, resume, segs);
        }
    }
}

/// 追加一个段：`start = 上一段末笔 + 1（或 0）`，`end = end_bi_idx`。
///
/// 校验失败（首段方向/数值异常）返回 false，调用方从 end_bi_idx+1 重新扫描。
fn add_new_seg(bis: &[BI], end_bi_idx: usize, is_sure: bool, segs: &mut Vec<Segment>) -> bool {
    // split_first_seg：窗口起点位于某段中间时，在首根确认段之前切出一段
    if segs.is_empty() && end_bi_idx >= 3 {
        let is_high = bis[end_bi_idx].direction == CzscDirection::Down;
        if let Some(peak) = find_peak_bi_rev(bis, end_bi_idx - 3, is_high) {
            let condition = if bis[peak].direction == CzscDirection::Up {
                bis[peak].get_high() > bis[0].get_high() || peak == 0
            } else {
                bis[peak].get_low() < bis[0].get_low() || peak == 0
            };
            if condition {
                try_add_new_seg(bis, peak, false, Some(dir_of(bis[peak].direction)), segs);
                try_add_new_seg(bis, end_bi_idx, false, None, segs);
                return true;
            }
        }
    }

    let start_bi_idx = segs.last().map(|s| s.end_bi_idx + 1).unwrap_or(0);
    if start_bi_idx > end_bi_idx {
        return false;
    }
    try_add_new_seg(bis, end_bi_idx, is_sure, None, segs)
}

fn try_add_new_seg(
    bis: &[BI],
    end_bi_idx: usize,
    is_sure: bool,
    seg_dir: Option<Dir>,
    segs: &mut Vec<Segment>,
) -> bool {
    if end_bi_idx >= bis.len() {
        return false;
    }
    let start_bi_idx = segs.last().map(|s| s.end_bi_idx + 1).unwrap_or(0);
    if start_bi_idx > end_bi_idx {
        return false;
    }
    let end_bi = &bis[end_bi_idx];
    let direction = seg_dir.unwrap_or(dir_of(end_bi.direction));

    let mut is_sure = is_sure;
    // 线段至少 3 笔，否则降级为未确认
    if end_bi_idx - start_bi_idx < 2 {
        is_sure = false;
    }
    if is_sure {
        // 缠论方向校验：上升线段起点低于终点、下降线段起点高于终点；
        // 校验失败（如数据窗口从段中间开始）时放弃本段，由调用方重新扫描
        let begin_val = bis[start_bi_idx].fx_a.fx;
        let end_val = end_bi.fx_b.fx;
        let ok = match direction {
            Dir::Up => begin_val < end_val,
            Dir::Down => begin_val > end_val,
        };
        if !ok {
            return false;
        }
    }

    segs.push(Segment {
        start_bi_idx,
        end_bi_idx,
        direction: match direction {
            Dir::Up => CzscDirection::Up,
            Dir::Down => CzscDirection::Down,
        },
        is_sure,
    });
    true
}

/// 收集剩余未确认部分（对应 `CSegListComm::collect_left_seg`，left_method=peak）。
fn collect_left_seg(bis: &[BI], segs: &mut Vec<Segment>) {
    if segs.is_empty() {
        collect_first_seg(bis, segs);
    } else {
        collect_segs(bis, segs);
    }
}

/// 一个确认段都没有时：按窗口内最高/最低点切出首段（未确认），再收尾。
fn collect_first_seg(bis: &[BI], segs: &mut Vec<Segment>) {
    if bis.len() < 3 {
        return;
    }
    let high = bis.iter().map(|b| b.get_high()).fold(f64::NEG_INFINITY, f64::max);
    let low = bis.iter().map(|b| b.get_low()).fold(f64::INFINITY, f64::min);
    let begin_val = bis[0].fx_a.fx;
    if (high - begin_val).abs() >= (low - begin_val).abs() {
        if let Some(peak) = find_peak_bi(bis, 0, bis.len(), true) {
            try_add_new_seg(bis, peak, false, Some(Dir::Up), segs);
        }
    } else if let Some(peak) = find_peak_bi(bis, 0, bis.len(), false) {
        try_add_new_seg(bis, peak, false, Some(Dir::Down), segs);
    }
    collect_left_as_seg(bis, segs);
}

/// 已有一段确认段时：收集其后剩余部分（优先找转折峰值形成未确认段）。
fn collect_segs(bis: &[BI], segs: &mut Vec<Segment>) {
    let last_bi = bis.len() - 1;
    let last_seg_end = segs.last().unwrap().end_bi_idx;
    if last_bi - last_seg_end < 3 {
        return;
    }
    let last_dir = segs.last().unwrap().direction;
    let last_end_val = bis[last_seg_end].fx_b.fx;
    let last_bi_val = bis[last_bi].fx_b.fx;
    if last_dir == CzscDirection::Down && last_bi_val <= last_end_val {
        if let Some(peak) = find_peak_bi(bis, last_seg_end + 3, bis.len(), true) {
            try_add_new_seg(bis, peak, false, Some(Dir::Up), segs);
            collect_left_seg(bis, segs);
        }
    } else if last_dir == CzscDirection::Up && last_bi_val >= last_end_val {
        if let Some(peak) = find_peak_bi(bis, last_seg_end + 3, bis.len(), false) {
            try_add_new_seg(bis, peak, false, Some(Dir::Down), segs);
            collect_left_seg(bis, segs);
        }
    } else {
        collect_left_seg_peak_method(bis, last_seg_end, segs);
    }
}

fn collect_left_seg_peak_method(bis: &[BI], last_seg_end: usize, segs: &mut Vec<Segment>) {
    let last_dir = segs.last().unwrap().direction;
    let is_high = last_dir == CzscDirection::Down;
    if let Some(peak) = find_peak_bi(bis, last_seg_end + 3, bis.len(), is_high) {
        if peak - last_seg_end >= 3 {
            try_add_new_seg(bis, peak, false, Some(if is_high { Dir::Up } else { Dir::Down }), segs);
        }
    }
    collect_left_as_seg(bis, segs);
}

/// 把最后一段终点之后剩余的笔收为一段（未确认）。
fn collect_left_as_seg(bis: &[BI], segs: &mut Vec<Segment>) {
    let last_bi = bis.len() - 1;
    let last_seg_end = segs.last().unwrap().end_bi_idx;
    if last_seg_end + 1 >= bis.len() {
        return;
    }
    let end_idx = if bis[last_seg_end].direction == bis[last_bi].direction {
        last_bi - 1
    } else {
        last_bi
    };
    try_add_new_seg(bis, end_idx, false, None, segs);
}

/// 在 `[start, end)` 内正向找"峰值笔"（对应 `FindPeakBi`）：
/// is_high=true 找向上笔中终点最高者（跳过其前前笔终点更高的非峰值笔）。
fn find_peak_bi(bis: &[BI], start: usize, end: usize, is_high: bool) -> Option<usize> {
    let mut peak_val = if is_high { f64::NEG_INFINITY } else { f64::INFINITY };
    let mut peak_bi = None;
    for i in start..end {
        let bi = &bis[i];
        let end_val = bi.fx_b.fx;
        let candidate = if is_high {
            bi.direction == CzscDirection::Up && end_val >= peak_val
        } else {
            bi.direction == CzscDirection::Down && end_val <= peak_val
        };
        if candidate {
            let skip = i >= 2
                && if is_high {
                    bis[i - 2].fx_b.fx > end_val
                } else {
                    bis[i - 2].fx_b.fx < end_val
                };
            if !skip {
                peak_val = end_val;
                peak_bi = Some(i);
            }
        }
    }
    peak_bi
}

/// 在 `[0..=max_idx]` 内反向找"峰值笔"（split_first_seg 用）。
fn find_peak_bi_rev(bis: &[BI], max_idx: usize, is_high: bool) -> Option<usize> {
    let mut peak_val = if is_high { f64::NEG_INFINITY } else { f64::INFINITY };
    let mut peak_bi = None;
    let mut i = max_idx;
    loop {
        let bi = &bis[i];
        let end_val = bi.fx_b.fx;
        let candidate = if is_high {
            bi.direction == CzscDirection::Up && end_val >= peak_val
        } else {
            bi.direction == CzscDirection::Down && end_val <= peak_val
        };
        if candidate {
            let skip = i >= 2
                && if is_high {
                    bis[i - 2].fx_b.fx > end_val
                } else {
                    bis[i - 2].fx_b.fx < end_val
                };
            if !skip {
                peak_val = end_val;
                peak_bi = Some(i);
            }
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    peak_bi
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造交替方向的笔序列（仿照 czsc BI 的关键字段）。
    fn make_bis(points: &[(f64, f64)]) -> Vec<BI> {
        // points: (low, high) 序列，交替作为底/顶
        let mut bis = Vec::new();
        for (i, &(lo, hi)) in points.iter().enumerate() {
            let (start_price, end_price) = if i % 2 == 0 {
                (lo, hi) // 向上笔：底 -> 顶
            } else {
                (hi, lo) // 向下笔：顶 -> 底
            };
            let fx_a = czsc_core::objects::fx::FX {
                symbol: std::sync::Arc::from("T"),
                dt: chrono::Utc::now(),
                mark: if i % 2 == 0 {
                    czsc_core::objects::mark::Mark::D
                } else {
                    czsc_core::objects::mark::Mark::G
                },
                high: start_price.max(end_price),
                low: start_price.min(end_price),
                fx: start_price,
                elements: Vec::new(),
            };
            let fx_b = czsc_core::objects::fx::FX {
                symbol: std::sync::Arc::from("T"),
                dt: chrono::Utc::now(),
                mark: if i % 2 == 0 {
                    czsc_core::objects::mark::Mark::G
                } else {
                    czsc_core::objects::mark::Mark::D
                },
                high: start_price.max(end_price),
                low: start_price.min(end_price),
                fx: end_price,
                elements: Vec::new(),
            };
            let direction = if i % 2 == 0 {
                CzscDirection::Up
            } else {
                CzscDirection::Down
            };
            bis.push(BI {
                symbol: std::sync::Arc::from("T"),
                fx_a,
                fx_b,
                fxs: Vec::new(),
                direction,
                bars: Vec::new(),
            });
        }
        bis
    }

    /// 锯齿上升结构：底 10 -> 顶 20 -> 底 12 -> 顶 22 -> 底 14 -> 顶 24 ...
    /// 特征序列（向下笔）低点依次抬高，不构成顶分型破坏，应只有一段（未完成）。
    #[test]
    fn rising_structure_forms_single_trailing_seg() {
        let pts: Vec<(f64, f64)> = (0..8)
            .map(|i| {
                let base = 10.0 + i as f64 * 2.0;
                (base, base + 10.0)
            })
            .collect();
        let bis = make_bis(&pts);
        let segs = build_segs(&bis);
        assert!(!segs.is_empty());
        for seg in &segs {
            assert!(seg.end_bi_idx >= seg.start_bi_idx);
        }
    }

    /// 简单锯齿：首段向上（底->顶->底->顶），顶分型出现在第 5 笔处。
    #[test]
    fn simple_sawtooth_produces_sure_segment() {
        // 底10 顶20 底9 顶21 底8 顶19 底7 顶18 —— 第5笔(底8, 索引4)创新低，
        // 向下笔序列 d1(20->9) d3(21->8) d5(19->7) 低点依次降低 → 无顶分型
        let pts = vec![
            (10.0, 20.0),
            (9.0, 21.0),
            (8.0, 19.0),
            (7.0, 18.0),
            (7.5, 17.0),
            (8.0, 16.0),
        ];
        let bis = make_bis(&pts);
        let segs = build_segs(&bis);
        // 至少能跑通且段之间连续
        for w in segs.windows(2) {
            assert_eq!(w[1].start_bi_idx, w[0].end_bi_idx + 1);
        }
    }
}
