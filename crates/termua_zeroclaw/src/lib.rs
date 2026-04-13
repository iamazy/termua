use std::{collections::HashMap, path::PathBuf};

use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct ProviderInfo {
    pub name: String,
    pub display_name: String,
    pub aliases: Vec<String>,
    pub local: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ClientOptions {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
    pub api_path: Option<String>,
    pub temperature: Option<f64>,
    pub provider_timeout_secs: Option<u64>,
    pub extra_headers: HashMap<String, String>,
}

pub struct Client;

#[derive(Clone, Debug)]
pub struct GatewayEndpoint {
    pub host: String,
    pub port: u16,
    pub path_prefix: Option<String>,
}

pub struct GatewayHandle {
    pub endpoint: GatewayEndpoint,
    join: Option<std::thread::JoinHandle<()>>,
}

impl GatewayHandle {
    pub fn join(mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[derive(Debug, Deserialize)]
struct DaemonStateFile {
    pid: Option<u32>,
}

fn default_api_url_for_provider(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("https://api.openai.com/v1"),
        // "codex" in zeroclaw maps to OpenAI's Codex API which is OpenAI-compatible.
        "codex" | "openai-codex" | "openai_codex" => Some("https://api.openai.com/v1"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        _ => None,
    }
}

fn models_url_from_base(api_url: &str) -> anyhow::Result<url::Url> {
    use anyhow::Context as _;

    let api_url = api_url.trim();
    if api_url.is_empty() {
        anyhow::bail!("api_url is required");
    }

    let mut base = api_url.to_string();
    if !base.ends_with('/') {
        base.push('/');
    }

    let base = url::Url::parse(&base).context("invalid api_url")?;
    base.join("models").context("join /models")
}

fn build_current_thread_runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    use anyhow::Context as _;

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")
}

fn load_config_blocking() -> anyhow::Result<zeroclaw::config::schema::Config> {
    use anyhow::Context as _;

    let rt = build_current_thread_runtime()?;
    rt.block_on(async {
        zeroclaw::config::schema::Config::load_or_init()
            .await
            .context("failed to load zeroclaw config")
    })
}

fn daemon_state_path_from_config_path(config_path: &std::path::Path) -> PathBuf {
    config_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("daemon_state.json")
}

fn endpoint_base_url(endpoint: &GatewayEndpoint) -> String {
    let mut base = format!("http://{}:{}", endpoint.host.trim(), endpoint.port);
    let pfx = endpoint
        .path_prefix
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or("");
    base.push_str(pfx);
    if !base.ends_with('/') {
        base.push('/');
    }
    base
}

impl Client {
    pub fn list_providers() -> Vec<ProviderInfo> {
        zeroclaw::providers::list_providers()
            .into_iter()
            .map(|p| ProviderInfo {
                name: p.name.to_string(),
                display_name: p.display_name.to_string(),
                aliases: p.aliases.iter().map(ToString::to_string).collect(),
                local: p.local,
            })
            .collect()
    }

    pub fn turn_blocking(prompt: String) -> anyhow::Result<String> {
        Self::turn_blocking_with_options(prompt, ClientOptions::default())
    }

    pub fn turn_blocking_with_options(
        prompt: String,
        options: ClientOptions,
    ) -> anyhow::Result<String> {
        use std::sync::Arc;

        use anyhow::Context as _;
        let rt = build_current_thread_runtime()?;

        rt.block_on(async move {
            use zeroclaw::{
                agent::{agent::Agent, dispatcher::NativeToolDispatcher},
                config::{MemoryConfig, schema::Config},
                memory,
                observability::NoopObserver,
                providers,
            };

            let config = Config::load_or_init()
                .await
                .context("failed to load zeroclaw config")?;

            let model_name = options
                .model
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .or_else(|| {
                    config
                        .default_model
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                })
                .context("zeroclaw config missing default_model")?;

            let provider_name = options
                .provider
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .or_else(|| {
                    config
                        .default_provider
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                })
                .context("zeroclaw config missing default_provider")?;

            let api_key = options
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .or_else(|| config.api_key.clone().filter(|s| !s.trim().is_empty()));

            let api_url = options
                .api_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .or_else(|| config.api_url.clone().filter(|s| !s.trim().is_empty()));

            // Create a provider matching the user's zeroclaw configuration.
            let mut runtime_opts = providers::provider_runtime_options_from_config(&config);
            if let Some(timeout) = options.provider_timeout_secs {
                runtime_opts.provider_timeout_secs = Some(timeout);
            }
            if let Some(api_path) = options
                .api_path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                runtime_opts.api_path = Some(api_path.to_string());
            }
            if let Some(url) = api_url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                // Needed for the "codex" provider because it reads base URL from runtime options.
                runtime_opts.provider_api_url = Some(url.to_string());
            }
            if !options.extra_headers.is_empty() {
                for (k, v) in options.extra_headers {
                    runtime_opts.extra_headers.insert(k, v);
                }
            }

            let provider = providers::create_routed_provider_with_options(
                &provider_name,
                api_key.as_deref(),
                api_url.as_deref(),
                &config.reliability,
                config.model_routes.as_slice(),
                model_name.as_str(),
                &runtime_opts,
            )
            .context("failed to create zeroclaw provider")?;

            // Read-only assistant: use an explicit no-op memory backend so AgentBuilder can
            // satisfy required wiring without persisting anything.
            let memory_cfg = MemoryConfig {
                backend: "none".into(),
                ..MemoryConfig::default()
            };
            let mem = Arc::from(
                memory::create_memory(&memory_cfg, &config.workspace_dir, api_key.as_deref())
                    .context("failed to create zeroclaw memory backend")?,
            );

            // Read-only: don't register any tools. The assistant only returns text suggestions.
            let mut agent = Agent::builder()
                .auto_save(false)
                .config(config.agent.clone())
                .identity_config(config.identity.clone())
                .temperature(options.temperature.unwrap_or(config.default_temperature))
                .workspace_dir(config.workspace_dir.clone())
                .model_name(model_name)
                .provider(provider)
                .tools(Vec::new())
                .memory(mem)
                .observer(Arc::from(NoopObserver {}))
                .tool_dispatcher(Box::new(NativeToolDispatcher))
                .build()
                .context("failed to build zeroclaw agent")?;

            agent
                .run_single(&prompt)
                .await
                .context("zeroclaw agent run_single failed")
        })
    }

    pub fn list_models_blocking_with_options(
        options: ClientOptions,
    ) -> anyhow::Result<Vec<String>> {
        use std::time::Duration;

        use anyhow::Context as _;

        let provider = options
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("openai");

        let api_url = options
            .api_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .or_else(|| default_api_url_for_provider(provider).map(ToString::to_string))
            .context("api_url is required to fetch models for this provider")?;

        let api_key = options
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .context("API key is required to fetch models")?;

        let timeout_secs = options.provider_timeout_secs.unwrap_or(30);
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(timeout_secs))
            .build();

        let models_url = models_url_from_base(&api_url)?;
        let mut req = agent
            .get(models_url.as_str())
            .set("accept", "application/json")
            .set("authorization", &format!("Bearer {api_key}"));

        for (k, v) in options.extra_headers.iter() {
            req = req.set(k, v);
        }

        let resp = req.call().context("fetch /models")?;
        let v: serde_json::Value = resp.into_json().context("parse /models json")?;

        fn extract_ids(arr: &[serde_json::Value]) -> Vec<String> {
            let mut out = Vec::new();
            for item in arr {
                if let Some(id) = item.as_str() {
                    let id = id.trim();
                    if !id.is_empty() {
                        out.push(id.to_string());
                    }
                    continue;
                }
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    let id = id.trim();
                    if !id.is_empty() {
                        out.push(id.to_string());
                    }
                }
            }
            out
        }

        let mut models = if let Some(arr) = v.get("data").and_then(|v| v.as_array()) {
            extract_ids(arr)
        } else if let Some(arr) = v.get("models").and_then(|v| v.as_array()) {
            extract_ids(arr)
        } else if let Some(arr) = v.as_array() {
            extract_ids(arr)
        } else {
            Vec::new()
        };

        models.sort();
        models.dedup();
        Ok(models)
    }

    pub fn gateway_endpoint_blocking() -> anyhow::Result<GatewayEndpoint> {
        let config = load_config_blocking()?;
        Ok(GatewayEndpoint {
            host: config.gateway.host,
            port: config.gateway.port,
            path_prefix: config.gateway.path_prefix,
        })
    }

    fn daemon_state_path_blocking() -> anyhow::Result<PathBuf> {
        let config = load_config_blocking()?;
        Ok(daemon_state_path_from_config_path(&config.config_path))
    }

    fn daemon_pid_blocking() -> anyhow::Result<Option<u32>> {
        use anyhow::Context as _;

        let path = Self::daemon_state_path_blocking()?;
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
        };

        let state: DaemonStateFile =
            serde_json::from_slice(&data).context("parse daemon_state.json")?;
        Ok(state.pid)
    }

    /// Best-effort: stop the external zeroclaw daemon (which supervises/restarts the gateway).
    ///
    /// Order:
    /// 1) Try `zeroclaw service stop` (stops systemd/launchd user service if installed).
    /// 2) Fall back to SIGTERM the PID in `daemon_state.json` (if it looks like a zeroclaw
    ///    process).
    pub fn stop_daemon_blocking() -> anyhow::Result<()> {
        // Prefer stopping the OS service if it's installed, to avoid restart loops.
        if let Ok(status) = std::process::Command::new("zeroclaw")
            .args(["service", "stop"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            && status.success()
        {
            return Ok(());
        }

        let Some(pid) = Self::daemon_pid_blocking()? else {
            // No daemon state file; assume it's not running.
            return Ok(());
        };

        let mut system = sysinfo::System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

        let Some(proc_) = system.process(sysinfo::Pid::from_u32(pid)) else {
            return Ok(());
        };

        let looks_like_zeroclaw = proc_
            .name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("zeroclaw")
            || proc_
                .exe()
                .and_then(|p| p.file_name())
                .and_then(|p| p.to_str())
                .is_some_and(|n| n.to_ascii_lowercase().contains("zeroclaw"));

        if !looks_like_zeroclaw {
            anyhow::bail!("refusing to stop pid {pid}: does not look like a zeroclaw process");
        }

        // Try graceful termination first.
        let sent = proc_.kill_with(sysinfo::Signal::Term).unwrap_or(false);
        if !sent {
            // Fall back to hard kill.
            let _ = proc_.kill();
        }

        // Wait briefly for it to exit.
        for _ in 0..20 {
            let mut system = sysinfo::System::new();
            system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
            if system.process(sysinfo::Pid::from_u32(pid)).is_none() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }

        anyhow::bail!("zeroclaw daemon pid {pid} did not exit after termination request");
    }

    pub fn gateway_health_blocking(endpoint: &GatewayEndpoint) -> anyhow::Result<bool> {
        let mut base = endpoint_base_url(endpoint);
        base.push_str("health");

        let resp = ureq::get(&base).set("accept", "application/json").call();
        match resp {
            Ok(r) => Ok(r.status() == 200),
            Err(ureq::Error::Status(code, _)) => Ok(code == 200),
            Err(err) => Err(anyhow::anyhow!(err)),
        }
    }

    pub fn gateway_shutdown_blocking(endpoint: &GatewayEndpoint) -> anyhow::Result<()> {
        use anyhow::Context as _;

        let mut base = endpoint_base_url(endpoint);
        base.push_str("admin/shutdown");

        let resp = ureq::post(&base)
            .set("accept", "application/json")
            .send_json(serde_json::json!({}))
            .context("POST /admin/shutdown")?;
        if resp.status() != 200 {
            anyhow::bail!("shutdown failed with status {}", resp.status());
        }
        Ok(())
    }

    pub fn gateway_start_background_blocking() -> anyhow::Result<GatewayHandle> {
        use anyhow::Context as _;

        let config = load_config_blocking()?;

        let endpoint = GatewayEndpoint {
            host: config.gateway.host.clone(),
            port: config.gateway.port,
            path_prefix: config.gateway.path_prefix.clone(),
        };

        let host = config.gateway.host.clone();
        let port = config.gateway.port;
        let join = std::thread::Builder::new()
            .name("termua-zeroclaw-gateway".to_string())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let Ok(rt) = rt else {
                    return;
                };
                rt.block_on(async move {
                    let _ = zeroclaw::gateway::run_gateway(&host, port, config).await;
                });
            })
            .context("spawn zeroclaw gateway thread")?;

        Ok(GatewayHandle {
            endpoint,
            join: Some(join),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_state_path_uses_config_parent_dir() {
        let config_path = PathBuf::from("/tmp/termua-zeroclaw/config/config.toml");
        assert_eq!(
            daemon_state_path_from_config_path(&config_path),
            PathBuf::from("/tmp/termua-zeroclaw/config/daemon_state.json")
        );
    }

    #[test]
    fn endpoint_base_url_trims_and_normalizes_prefix() {
        let endpoint = GatewayEndpoint {
            host: "127.0.0.1".to_string(),
            port: 7231,
            path_prefix: Some(" /api ".to_string()),
        };
        assert_eq!(endpoint_base_url(&endpoint), "http://127.0.0.1:7231/api/");
    }
}
