#[derive(Debug, Clone)]
pub struct AppConfig {
    pub protocol_version: String,
    pub service_version: String,
    pub czsc_version: String,
    pub pipe_name: String,
    pub max_bi_num: u32,
    pub min_bi_len: u32,
    /// 默认分析窗口：**0 = 全量**（分析通达信传入的全部 K 线，前方历史
    /// 全部参与结构与信号计算；配合增量缓存，序列增长/盘中原地更新走
    /// 增量承接，稳态不重放窗口）。如需限窗可显式配置为正数
    /// （取尾部 N 根——注意固定尾窗口的序列增长会回退全量分析）。
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
    /// 服务端同时执行的全量分析并发上限（P1 参数化）：多窗口/多周期/
    /// 多标的场景可调大（如 4~8）；增量承接与缓存命中不占槽位。
    pub analysis_parallelism: usize,
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
            max_bi_num: 1000,
            min_bi_len: 6,
            default_analysis_window: 0,
            cache_capacity: 64,
            enable_incremental: true,
            incremental_cache_capacity: 8,
            analysis_parallelism: 4,
            derive_volume_proxy: true,
        }
    }
}

/// 服务端配置文件路径：与可执行文件同目录的 `tdx_czsc_service.ini`。
///
/// 与 Bridge DLL 的 `tdx_czsc_bridge.ini` 同目录同风格（UTF-8、`key = value`、
/// `#`/`;` 注释），部署时两者可放同一目录（如通达信 `T0002/dlls/`）。
const INI_FILE_NAME: &str = "tdx_czsc_service.ini";

/// 配置加载（优先级低到高）：内置默认值 → INI 文件 → 环境变量 `TDX_CZSC_*`。
///
/// INI 不存在时静默回退默认值（服务照常运行）；未知键忽略（与 DLL 端
/// 行为一致）；布尔键接受 `true/false/1/0`（大小写不敏感）。
impl AppConfig {
    pub fn load() -> Self {
        let mut config = Self::default();
        if let Some(ini_path) = default_ini_path()
            && let Ok(text) = std::fs::read_to_string(ini_path)
        {
            config.apply_ini_text(&text);
        }
        config.apply_env();
        config
    }

    fn apply_ini_text(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "pipe_name" => self.pipe_name = value.to_string(),
                "max_bi_num" => set_u32(&mut self.max_bi_num, value),
                "min_bi_len" => set_u32(&mut self.min_bi_len, value),
                "analysis_window" => set_u32(&mut self.default_analysis_window, value),
                "cache_capacity" => set_usize(&mut self.cache_capacity, value),
                "enable_incremental" => set_bool(&mut self.enable_incremental, value),
                "incremental_cache_capacity" => {
                    set_usize(&mut self.incremental_cache_capacity, value);
                }
                "analysis_parallelism" => set_usize(&mut self.analysis_parallelism, value),
                "derive_volume_proxy" => set_bool(&mut self.derive_volume_proxy, value),
                _ => {} // 未知键忽略
            }
        }
    }

    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("TDX_CZSC_PIPE_NAME") {
            self.pipe_name = v;
        }
        env_u32(&mut self.max_bi_num, "TDX_CZSC_MAX_BI_NUM");
        env_u32(&mut self.min_bi_len, "TDX_CZSC_MIN_BI_LEN");
        env_u32(
            &mut self.default_analysis_window,
            "TDX_CZSC_ANALYSIS_WINDOW",
        );
        env_usize(&mut self.cache_capacity, "TDX_CZSC_CACHE_CAPACITY");
        env_bool(&mut self.enable_incremental, "TDX_CZSC_ENABLE_INCREMENTAL");
        env_usize(
            &mut self.incremental_cache_capacity,
            "TDX_CZSC_INCREMENTAL_CACHE_CAPACITY",
        );
        env_usize(
            &mut self.analysis_parallelism,
            "TDX_CZSC_ANALYSIS_PARALLELISM",
        );
        env_bool(
            &mut self.derive_volume_proxy,
            "TDX_CZSC_DERIVE_VOLUME_PROXY",
        );
    }
}

fn default_ini_path() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|dir| dir.join(INI_FILE_NAME))
}

fn set_u32(target: &mut u32, value: &str) {
    if let Ok(v) = value.parse() {
        *target = v;
    }
}

fn set_usize(target: &mut usize, value: &str) {
    if let Ok(v) = value.parse() {
        *target = v;
    }
}

fn set_bool(target: &mut bool, value: &str) {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" => *target = true,
        "false" | "0" => *target = false,
        _ => {}
    }
}

fn env_u32(target: &mut u32, name: &str) {
    if let Ok(v) = std::env::var(name) {
        set_u32(target, &v);
    }
}

fn env_usize(target: &mut usize, name: &str) {
    if let Ok(v) = std::env::var(name) {
        set_usize(target, &v);
    }
}

fn env_bool(target: &mut bool, name: &str) {
    if let Ok(v) = std::env::var(name) {
        set_bool(target, &v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_text_overrides_defaults_and_ignores_unknown() {
        let mut config = AppConfig::default();
        config.apply_ini_text(
            "# 注释行
; 分号注释
pipe_name = \\\\.\\pipe\\tdx_czsc_test
max_bi_num = 500
analysis_window = 0
enable_incremental = false
derive_volume_proxy = 1
unknown_key = whatever
broken line without equals",
        );

        assert_eq!(config.pipe_name, r"\\.\pipe\tdx_czsc_test");
        assert_eq!(config.max_bi_num, 500);
        assert_eq!(config.default_analysis_window, 0);
        assert!(!config.enable_incremental);
        assert!(config.derive_volume_proxy);
        // 未出现的键保持默认值
        assert_eq!(config.min_bi_len, 6);
        assert_eq!(config.analysis_parallelism, 4);
    }

    #[test]
    fn invalid_values_keep_defaults() {
        let mut config = AppConfig::default();
        config
            .apply_ini_text("max_bi_num = not_a_number\nmin_bi_len =\nenable_incremental = maybe");
        assert_eq!(config.max_bi_num, 1000);
        assert_eq!(config.min_bi_len, 6);
        assert!(config.enable_incremental);
    }

    #[test]
    fn load_without_ini_falls_back_to_defaults() {
        // 测试进程的 exe 目录下不会有 tdx_czsc_service.ini（从未创建），
        // load 必须静默回退默认值。
        let config = AppConfig::load();
        assert_eq!(config.pipe_name, r"\\.\pipe\tdx_czsc_analyzer");
        assert_eq!(config.max_bi_num, 1000);
    }
}
