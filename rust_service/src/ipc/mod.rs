use anyhow::Result;

use crate::state::AppState;

#[cfg(windows)]
use anyhow::Context;
#[cfg(windows)]
use prost::Message;

#[cfg(windows)]
use crate::handlers;
#[cfg(windows)]
use crate::proto::analysis::RequestEnvelope;
#[cfg(windows)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(windows)]
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
#[cfg(windows)]
use tracing::{info, warn};

pub async fn run_server(state: std::sync::Arc<AppState>) -> Result<()> {
    #[cfg(windows)]
    {
        info!(pipe = %state.config.pipe_name, "starting named pipe server");
        run_windows_named_pipe_server(state).await
    }

    #[cfg(not(windows))]
    {
        let _ = state;
        Err(anyhow::anyhow!(
            "named pipe server is only supported on Windows hosts"
        ))
    }
}

#[cfg(windows)]
async fn run_windows_named_pipe_server(state: std::sync::Arc<AppState>) -> Result<()> {
    loop {
        let server = match ServerOptions::new().create(&state.config.pipe_name) {
            Ok(server) => server,
            Err(err) => {
                // 管道创建失败（如名称冲突）时短暂退避后重试，不终止服务。
                warn!(
                    error = %err,
                    pipe = %state.config.pipe_name,
                    "failed to create named pipe, retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
        };

        // 客户端可能在握手期间断开（如 DLL 探测连接后立即关闭），
        // connect 失败属于常态，继续监听而非终止服务。
        if let Err(err) = server.connect().await {
            warn!(
                error = %err,
                pipe = %state.config.pipe_name,
                "named pipe connect failed, waiting for next client"
            );
            continue;
        }

        let state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_client(server, state).await {
                // 客户端（DLL）读超时/断开属于常态（首次分析超时、通达信关闭），
                // 用 warn 而非 error 记录，避免刷屏。
                warn!(error = %err, "named pipe client handling ended");
            }
        });
    }
}

#[cfg(windows)]
async fn handle_client(mut server: NamedPipeServer, state: std::sync::Arc<AppState>) -> Result<()> {
    let request_bytes = read_frame(&mut server).await?;
    let request =
        RequestEnvelope::decode(request_bytes.as_ref()).context("decode request failed")?;

    // 错误已由 handlers 映射为协议状态码，这里不再需要 Err 分支。
    let response = handlers::handle_request(&state, request).await;

    let mut buf = Vec::with_capacity(response.encoded_len() + 4);
    response
        .encode(&mut buf)
        .context("encode response failed")?;
    write_frame(&mut server, &buf).await?;
    server
        .flush()
        .await
        .context("flush named pipe response failed")?;
    Ok(())
}

#[cfg(windows)]
async fn read_frame(server: &mut NamedPipeServer) -> Result<Vec<u8>> {
    let len = server
        .read_u32_le()
        .await
        .context("failed to read frame length")? as usize;

    let mut buf = vec![0u8; len];
    server
        .read_exact(&mut buf)
        .await
        .context("failed to read frame payload")?;
    Ok(buf)
}

#[cfg(windows)]
async fn write_frame(server: &mut NamedPipeServer, payload: &[u8]) -> Result<()> {
    server
        .write_u32_le(payload.len() as u32)
        .await
        .context("failed to write frame length")?;
    server
        .write_all(payload)
        .await
        .context("failed to write frame payload")?;
    Ok(())
}
