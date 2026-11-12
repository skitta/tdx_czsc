use thiserror::Error;

/// 配置加载/校验错误：非法值不再静默回退默认值。
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid config value for {key}: {value}")]
    InvalidValue { key: String, value: String },
    #[error("config value {key} must be greater than zero")]
    MustBePositive { key: &'static str },
}

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
/// 行为一致）。取值一律走 `FromStr`：数值必须落在目标整型范围内，布尔键
/// 只接受小写 `true`/`false`（`1`/`0` 不是布尔值）。
impl AppConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let mut config = Self::default();
        if let Some(ini_path) = Self::ini_path() {
            match std::fs::read_to_string(&ini_path) {
                Ok(text) => config.apply_ini_text(&text)?,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(ConfigError::Io(err)),
            }
        }
        config.apply_env()?;
        config.validate()?;
        Ok(config)
    }

    /// INI 文件路径：与 exe 同目录。
    fn ini_path() -> Option<std::path::PathBuf> {
        std::env::current_exe()
            .ok()?
            .parent()
            .map(|dir| dir.join(INI_FILE_NAME))
    }

    /// 校验会影响运行时不变量的配置。容量为 0 表示禁用对应缓存，不是错误。
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.max_bi_num == 0 {
            return Err(ConfigError::MustBePositive { key: "max_bi_num" });
        }
        if self.min_bi_len == 0 {
            return Err(ConfigError::MustBePositive { key: "min_bi_len" });
        }
        if self.analysis_parallelism == 0 {
            return Err(ConfigError::MustBePositive {
                key: "analysis_parallelism",
            });
        }
        Ok(())
    }

    fn apply_ini_text(&mut self, text: &str) -> Result<(), ConfigError> {
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
                "max_bi_num" => Field::new(key, &mut self.max_bi_num).set(value)?,
                "min_bi_len" => Field::new(key, &mut self.min_bi_len).set(value)?,
                "analysis_window" => {
                    Field::new(key, &mut self.default_analysis_window).set(value)?;
                }
                "cache_capacity" => Field::new(key, &mut self.cache_capacity).set(value)?,
                "enable_incremental" => Field::new(key, &mut self.enable_incremental).set(value)?,
                "incremental_cache_capacity" => {
                    Field::new(key, &mut self.incremental_cache_capacity).set(value)?;
                }
                "analysis_parallelism" => {
                    Field::new(key, &mut self.analysis_parallelism).set(value)?;
                }
                "derive_volume_proxy" => {
                    Field::new(key, &mut self.derive_volume_proxy).set(value)?
                }
                _ => {} // 未知键忽略
            }
        }
        Ok(())
    }

    fn apply_env(&mut self) -> Result<(), ConfigError> {
        if let Ok(value) = std::env::var("TDX_CZSC_PIPE_NAME") {
            self.pipe_name = value;
        }
        Field::new("max_bi_num", &mut self.max_bi_num).set_from_env("TDX_CZSC_MAX_BI_NUM")?;
        Field::new("min_bi_len", &mut self.min_bi_len).set_from_env("TDX_CZSC_MIN_BI_LEN")?;
        Field::new("analysis_window", &mut self.default_analysis_window)
            .set_from_env("TDX_CZSC_ANALYSIS_WINDOW")?;
        Field::new("cache_capacity", &mut self.cache_capacity)
            .set_from_env("TDX_CZSC_CACHE_CAPACITY")?;
        Field::new("enable_incremental", &mut self.enable_incremental)
            .set_from_env("TDX_CZSC_ENABLE_INCREMENTAL")?;
        Field::new(
            "incremental_cache_capacity",
            &mut self.incremental_cache_capacity,
        )
        .set_from_env("TDX_CZSC_INCREMENTAL_CACHE_CAPACITY")?;
        Field::new("analysis_parallelism", &mut self.analysis_parallelism)
            .set_from_env("TDX_CZSC_ANALYSIS_PARALLELISM")?;
        Field::new("derive_volume_proxy", &mut self.derive_volume_proxy)
            .set_from_env("TDX_CZSC_DERIVE_VOLUME_PROXY")?;
        Ok(())
    }
}

/// 单个配置字段的解析句柄：把「字段名 + 可变目标」绑成零成本抽象
/// （单态化，无分配），调用点只描述取值来源。
struct Field<'a, T> {
    key: &'a str,
    target: &'a mut T,
}

impl<'a, T: std::str::FromStr> Field<'a, T> {
    fn new(key: &'a str, target: &'a mut T) -> Self {
        Self { key, target }
    }

    /// INI 取值：解析成功才写回，失败时目标字段保持原值。
    fn set(self, raw: &str) -> Result<(), ConfigError> {
        *self.target = raw.trim().parse().map_err(|_| ConfigError::InvalidValue {
            key: self.key.to_string(),
            value: raw.to_string(),
        })?;
        Ok(())
    }

    /// 环境变量取值：未设置不覆盖；非 Unicode 视为非法值。
    fn set_from_env(self, env: &str) -> Result<(), ConfigError> {
        match std::env::var(env) {
            Ok(raw) => self.set(&raw),
            Err(std::env::VarError::NotPresent) => Ok(()),
            Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::InvalidValue {
                key: env.to_string(),
                value: "<non-unicode>".to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_text_overrides_defaults_and_ignores_unknown() {
        let mut config = AppConfig::default();
        config
            .apply_ini_text(
                "# 注释行
; 分号注释
pipe_name = \\\\.\\pipe\\tdx_czsc_test
max_bi_num = 500
analysis_window = 0
enable_incremental = false
derive_volume_proxy = true
unknown_key = whatever
broken line without equals",
            )
            .expect("valid ini");

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
    fn invalid_values_are_reported() {
        let mut config = AppConfig::default();
        let err = config
            .apply_ini_text("max_bi_num = not_a_number\nmin_bi_len =\nenable_incremental = maybe");
        assert!(matches!(err, Err(ConfigError::InvalidValue { .. })));
        assert_eq!(config.max_bi_num, 1000);
        assert_eq!(config.min_bi_len, 6);
        assert!(config.enable_incremental);
    }

    #[test]
    fn bool_values_reject_numeric_tokens() {
        let mut config = AppConfig::default();
        for token in ["1", "0", "yes", "TRUE"] {
            let text = format!("enable_incremental = {token}");
            let err = config.apply_ini_text(&text);
            assert!(
                matches!(err, Err(ConfigError::InvalidValue { .. })),
                "{token} 不应被接受为布尔值"
            );
            assert!(config.enable_incremental, "解析失败必须保留原值");
        }
    }

    #[test]
    fn load_without_ini_falls_back_to_defaults() {
        // 测试进程的 exe 目录下不会有 tdx_czsc_service.ini（从未创建），
        // load 必须静默回退默认值。
        let config = AppConfig::load().expect("load defaults");
        assert_eq!(config.pipe_name, r"\\.\pipe\tdx_czsc_analyzer");
        assert_eq!(config.max_bi_num, 1000);
    }
}
