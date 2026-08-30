#[derive(Debug, Clone)]
pub struct AppConfig {
    pub protocol_version: String,
    pub service_version: String,
    pub czsc_version: String,
    pub pipe_name: String,
    pub request_timeout_ms: u64,
    pub max_bi_num: u32,
    pub min_bi_len: u32,
    pub default_analysis_window: u32,
    pub cache_capacity: usize,
    /// 增量分析：开启后服务端缓存已演化的 CZSC 与信号评估器，新请求与
    /// 缓存序列前缀一致时只演化新增 K 线（窗口首部稳定时稳态刷新
    /// O(新增 K 线)，见 `analyzer/incremental.rs`）；不适用时自动回退
    /// 全量分析，输出与全量路径完全一致。
    pub enable_incremental: bool,
    /// 增量状态缓存容量（每条目持有演化中的 CZSC，含完整窗口 K 线；
    /// 大窗口下按内存敏感配置调小）。
    pub incremental_cache_capacity: usize,
    /// 通达信公式未挂量能通道（func_id 950）时，vol 缺失导致一买/一卖
    /// 的"量能背驰"恒不成立。开启后以单根K线相对振幅 (high-low)/close×100
    /// 派生代理量，恢复"价格背驰 AND（量能 OR 长度）"判定；有真实
    /// `MarketSeries.vol`（950 通道/CSV 工具链）时始终优先使用真实量。
    pub derive_volume_proxy: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            protocol_version: "1.0.0".to_string(),
            service_version: env!("CARGO_PKG_VERSION").to_string(),
            czsc_version: "czsc-core".to_string(),
            pipe_name: r"\\.\pipe\tdx_czsc_analyzer".to_string(),
            request_timeout_ms: 50,
            max_bi_num: 50,
            min_bi_len: 6,
            default_analysis_window: 1000,
            cache_capacity: 64,
            enable_incremental: true,
            incremental_cache_capacity: 8,
            derive_volume_proxy: true,
        }
    }
}
