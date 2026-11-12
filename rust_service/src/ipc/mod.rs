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
use tokio::sync::Semaphore;
#[cfg(windows)]
use tracing::{info, warn};

#[cfg(windows)]
const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;
#[cfg(windows)]
const MAX_CONNECTIONS: usize = 64;
#[cfg(windows)]
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Named Pipe 服务生命周期入口。
pub struct IpcServer {
    state: std::sync::Arc<AppState>,
}

impl IpcServer {
    pub fn new(state: std::sync::Arc<AppState>) -> Self {
        Self { state }
    }

    pub async fn run(self) -> Result<()> {
        #[cfg(windows)]
        {
            info!(pipe = %self.state.config().pipe_name, "starting named pipe server");
            self.serve().await
        }

        #[cfg(not(windows))]
        {
            let _ = self.state;
            Err(anyhow::anyhow!(
                "named pipe server is only supported on Windows hosts"
            ))
        }
    }

    #[cfg(windows)]
    async fn serve(self) -> Result<()> {
        let connections = std::sync::Arc::new(Semaphore::new(MAX_CONNECTIONS));
        loop {
            let server = match ServerOptions::new().create(&self.state.config().pipe_name) {
                Ok(server) => server,
                Err(err) => {
                    // 管道创建失败（如名称冲突）时短暂退避后重试，不终止服务。
                    warn!(
                        error = %err,
                        pipe = %self.state.config().pipe_name,
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
                    pipe = %self.state.config().pipe_name,
                    "named pipe connect failed, waiting for next client"
                );
                continue;
            }

            let permit = connections
                .clone()
                .acquire_owned()
                .await
                .context("connection semaphore closed")?;
            let state = self.state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let mut connection = IpcConnection::new(server);
                if let Err(err) = connection.handle(state).await {
                    // 客户端（DLL）读超时/断开属于常态（首次分析超时、通达信关闭），
                    // 用 warn 而非 error 记录，避免刷屏。
                    warn!(error = %err, "named pipe client handling ended");
                }
            });
        }
    }
}

/// 单条 Named Pipe 连接：负责长度前缀帧读写与请求分派。
#[cfg(windows)]
struct IpcConnection {
    server: NamedPipeServer,
}

#[cfg(windows)]
impl IpcConnection {
    fn new(server: NamedPipeServer) -> Self {
        Self { server }
    }

    async fn handle(&mut self, state: std::sync::Arc<AppState>) -> Result<()> {
        let request_bytes = tokio::time::timeout(REQUEST_TIMEOUT, self.read_frame())
            .await
            .context("request read timed out")??;
        let request =
            RequestEnvelope::decode(request_bytes.as_ref()).context("decode request failed")?;

        // 错误已由 handlers 映射为协议状态码，这里不再需要 Err 分支。
        let response = handlers::Handlers::new(state).dispatch(request).await;

        let mut buf = Vec::with_capacity(response.encoded_len() + 4);
        response
            .encode(&mut buf)
            .context("encode response failed")?;
        self.write_frame(&buf).await?;
        self.server
            .flush()
            .await
            .context("flush named pipe response failed")?;
        Ok(())
    }

    async fn read_frame(&mut self) -> Result<Vec<u8>> {
        let len = self
            .server
            .read_u32_le()
            .await
            .context("failed to read frame length")? as usize;
        if len > MAX_FRAME_LEN {
            anyhow::bail!("frame length {len} exceeds maximum {MAX_FRAME_LEN}");
        }

        let mut buf = vec![0u8; len];
        self.server
            .read_exact(&mut buf)
            .await
            .context("failed to read frame payload")?;
        Ok(buf)
    }

    async fn write_frame(&mut self, payload: &[u8]) -> Result<()> {
        let len = u32::try_from(payload.len()).context("response frame exceeds u32::MAX")?;
        self.server
            .write_u32_le(len)
            .await
            .context("failed to write frame length")?;
        self.server
            .write_all(payload)
            .await
            .context("failed to write frame payload")?;
        Ok(())
    }
}
