use std::path::PathBuf;

use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use tdx_czsc_service::config::AppConfig;
use tdx_czsc_service::ipc;
use tdx_czsc_service::state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    let log_path = parse_log_arg();
    init_tracing(log_path);

    let config = AppConfig::load();
    let state = AppState::new(config.clone());

    info!(
        service_version = %config.service_version,
        protocol_version = %config.protocol_version,
        pipe_name = %config.pipe_name,
        "tdx_czsc_service starting"
    );

    if let Err(err) = ipc::run_server(state).await {
        error!(error = %err, "service stopped with error");
        return Err(err);
    }

    Ok(())
}

/// 解析 `--log <路径>`：Bridge DLL 拉起服务时指定，服务日志写入该文件；
/// 手动运行时可不带，日志输出到控制台。
fn parse_log_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--log" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

fn init_tracing(log_path: Option<PathBuf>) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tdx_czsc_service=debug"));

    if let Some(path) = log_path {
        // 追加写文件；关闭 ANSI 颜色码，避免日志文件里出现转义序列。
        // 打不开（目录只读等）时降级为 stdout，服务照常运行。
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .compact()
                .with_writer(std::sync::Mutex::new(file))
                .with_ansi(false)
                .init();
            return;
        }
        eprintln!(
            "warning: cannot open --log file {}, fallback to stdout",
            path.display()
        );
    }

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
