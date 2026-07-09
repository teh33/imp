use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::client::{LightpandaClient, McpOutput};
use super::config::BrowserConfig;
use super::BrowserSessionId;

pub(crate) struct BrowserSession {
    pub(crate) client: LightpandaClient,
    pub(crate) last_used: Instant,
}

pub(crate) struct BrowserSessionManager {
    sessions: HashMap<BrowserSessionId, BrowserSession>,
}

impl BrowserSessionManager {
    pub(crate) fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub(crate) async fn start(
        &mut self,
        config: &BrowserConfig,
        cwd: &std::path::Path,
    ) -> Result<BrowserSessionId, String> {
        self.prune_idle(config.idle_timeout_seconds).await;
        if self.sessions.len() >= config.max_sessions {
            return Err(format!(
                "browser session limit reached (max {})",
                config.max_sessions
            ));
        }
        let id = BrowserSessionId::new();
        let client = LightpandaClient::spawn(config, cwd).await?;
        self.sessions.insert(
            id.clone(),
            BrowserSession {
                client,
                last_used: Instant::now(),
            },
        );
        Ok(id)
    }

    pub(crate) async fn call(
        &mut self,
        id: &BrowserSessionId,
        tool: &str,
        arguments: serde_json::Value,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
        timeout: Duration,
    ) -> Result<McpOutput, String> {
        let session = self
            .sessions
            .get_mut(id)
            .ok_or_else(|| format!("browser session `{}` was not found", id.as_str()))?;
        session.last_used = Instant::now();
        let output = session
            .client
            .call(tool, arguments, cancelled, timeout)
            .await;
        if output.is_err() {
            if let Some(mut failed) = self.sessions.remove(id) {
                failed.client.abort();
            }
        }
        output.map_err(|error| format!("{error}; browser session terminated"))
    }

    pub(crate) async fn stop(&mut self, id: &BrowserSessionId) -> Result<(), String> {
        let session = self
            .sessions
            .remove(id)
            .ok_or_else(|| format!("browser session `{}` was not found", id.as_str()))?;
        session.client.shutdown().await
    }

    pub(crate) fn abort_all(&mut self) {
        for session in self.sessions.values_mut() {
            session.client.abort();
        }
        self.sessions.clear();
    }

    async fn prune_idle(&mut self, idle_seconds: u64) {
        let cutoff = Duration::from_secs(idle_seconds);
        let stale = self
            .sessions
            .iter()
            .filter(|(_, session)| session.last_used.elapsed() >= cutoff)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in stale {
            if let Some(session) = self.sessions.remove(&id) {
                let _ = session.client.shutdown().await;
            }
        }
    }
}

impl Default for BrowserSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BrowserSessionManager {
    fn drop(&mut self) {
        self.abort_all();
    }
}
