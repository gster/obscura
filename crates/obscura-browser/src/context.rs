use std::path::PathBuf;
use std::sync::Arc;

use crate::network_history::NetworkHistory;
use obscura_net::{CookieJar, EffectivePersona, ObscuraHttpClient, RobotsCache};

#[derive(Clone, Debug, Default)]
pub struct BrowserContextOptions {
    pub proxy_url: Option<String>,
    pub storage_dir: Option<PathBuf>,
    pub allow_file_access: bool,
    pub allow_private_network: bool,
    pub obey_robots: bool,
}

impl Drop for BrowserContext {
    fn drop(&mut self) {
        // Pages retain an Arc<BrowserContext>, so the final context drop only
        // runs after every Page writer has closed and flushed its accepted
        // prefix. Persist the context-close marker for clean recovery.
        let _ = self.network_history.finalize();
    }
}

pub struct BrowserContext {
    pub id: String,
    pub cookie_jar: Arc<CookieJar>,
    pub local_storage: obscura_js::ops::SharedWebStorage,
    pub http_client: Arc<ObscuraHttpClient>,
    persona: Arc<EffectivePersona>,
    pub proxy_url: Option<String>,
    pub robots_cache: Arc<RobotsCache>,
    pub obey_robots: bool,
    pub allow_file_access: bool,
    pub storage_dir: Option<PathBuf>,
    pub allow_private_network: bool,
    network_history: NetworkHistory,
    network_observation_root: std::sync::Mutex<obscura_js::network_observation::NetworkObservationQueue>,
}

impl BrowserContext {
    pub fn try_new(id: String, persona: EffectivePersona) -> Result<Self, obscura_net::PersonaError> {
        Self::try_with_options(id, persona, BrowserContextOptions::default())
    }

    pub fn new(id: String, persona: EffectivePersona) -> Self {
        Self::try_new(id, persona).expect("browser context persona conflicts with process identity")
    }

    pub fn try_with_proxy(
        id: String,
        persona: EffectivePersona,
        proxy_url: Option<String>,
    ) -> Result<Self, obscura_net::PersonaError> {
        Self::try_with_options(
            id,
            persona,
            BrowserContextOptions {
                proxy_url,
                ..BrowserContextOptions::default()
            },
        )
    }

    pub fn with_proxy(
        id: String,
        persona: EffectivePersona,
        proxy_url: Option<String>,
    ) -> Self {
        Self::try_with_proxy(id, persona, proxy_url)
            .expect("browser context persona conflicts with process identity")
    }

    pub fn try_with_storage_and_network(
        id: String,
        persona: EffectivePersona,
        proxy_url: Option<String>,
        storage_dir: Option<PathBuf>,
        allow_private_network: bool,
    ) -> Result<Self, obscura_net::PersonaError> {
        Self::try_with_options(
            id,
            persona,
            BrowserContextOptions {
                proxy_url,
                storage_dir,
                allow_file_access: false,
                allow_private_network,
                obey_robots: false,
            },
        )
    }

    pub fn with_storage_and_network(
        id: String,
        persona: EffectivePersona,
        proxy_url: Option<String>,
        storage_dir: Option<PathBuf>,
        allow_private_network: bool,
    ) -> Self {
        Self::try_with_storage_and_network(
            id,
            persona,
            proxy_url,
            storage_dir,
            allow_private_network,
        )
        .expect("browser context persona conflicts with process identity")
    }

    /// Create a usable context from one already-validated persona snapshot.
    /// Network, storage, and privacy policy remain independent options.
    pub fn with_options(
        id: String,
        persona: EffectivePersona,
        options: BrowserContextOptions,
    ) -> Self {
        Self::try_with_options(id, persona, options)
            .expect("browser context persona conflicts with process identity")
    }

    /// Checked construction boundary for embedders. This freezes the
    /// process-wide timezone and primary locale before a usable context can be
    /// returned.
    pub fn try_with_options(
        id: String,
        persona: EffectivePersona,
        options: BrowserContextOptions,
    ) -> Result<Self, obscura_net::PersonaError> {
        obscura_net::activate_process_persona(&persona)?;
        let BrowserContextOptions {
            proxy_url,
            storage_dir,
            allow_file_access,
            allow_private_network,
            obey_robots,
        } = options;
        let cookie_jar = Arc::new(CookieJar::new());

        if let Some(ref dir) = storage_dir {
            let cookie_path = dir.join("cookies.json");
            if cookie_path.exists() {
                match cookie_jar.load_from_file(&cookie_path) {
                    Ok(n) if n > 0 => {
                        tracing::info!("Loaded {} cookies from {}", n, cookie_path.display());
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(
                            "Failed to load cookies from {}: {}",
                            cookie_path.display(),
                            error
                        );
                    }
                }
            }
        }

        let mut client = ObscuraHttpClient::with_full_options(
            cookie_jar.clone(),
            proxy_url.as_deref(),
            allow_private_network,
        );
        client.block_trackers = true;
        if let Ok(mut guard) = client.user_agent.try_write() {
            *guard = persona.user_agent().to_string();
        }

        let network_history = match storage_dir.as_deref() {
            Some(directory) => NetworkHistory::persistent(id.clone(), directory)
                .unwrap_or_else(|error| NetworkHistory::failed(id.clone(), error)),
            None => NetworkHistory::ephemeral(id.clone()),
        };
        let network_observation_root =
            obscura_js::network_observation::NetworkObservationQueue::default();
        if let Some(failure) = network_history.terminal_failure() {
            network_observation_root.fail_history(failure.message);
        }

        Ok(Self {
            id,
            cookie_jar,
            local_storage: Default::default(),
            http_client: Arc::new(client),
            persona: Arc::new(persona),
            proxy_url,
            robots_cache: Arc::new(RobotsCache::new()),
            obey_robots,
            allow_file_access,
            storage_dir,
            allow_private_network,
            network_history,
            network_observation_root: std::sync::Mutex::new(network_observation_root),
        })
    }

    pub fn persona(&self) -> &EffectivePersona {
        &self.persona
    }

    pub fn network_history(&self) -> NetworkHistory {
        self.network_history.clone()
    }

    pub(crate) fn network_observation_queue(
        &self,
    ) -> obscura_js::network_observation::NetworkObservationQueue {
        let root = self.network_observation_root
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(failure) = self.network_history.terminal_failure() {
            root.fail_history(failure.message);
        }
        root.sibling()
    }

    pub(crate) fn propagate_network_history_terminal(
        &self,
    ) -> Option<crate::network_history::NetworkHistoryError> {
        let failure = self.network_history.terminal_failure()?;
        self.network_observation_root
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .fail_history(failure.message.clone());
        Some(failure)
    }

    pub fn device_identity(&self) -> obscura_js::ops::DeviceIdentity {
        obscura_js::ops::DeviceIdentity {
            seed: self.persona.seed(),
            hardware_concurrency: self.persona.hardware_concurrency(),
            device_memory: self.persona.device_memory(),
            screen_width: self.persona.screen_width(),
            screen_height: self.persona.screen_height(),
        }
    }

    /// Copy the immutable persona while isolating cookies, storage, policy
    /// clients, and connection pools.
    pub fn isolated_copy(&self, id: String, persistent: bool) -> Self {
        self.isolated_copy_with_persona(id, persistent, (*self.persona).clone())
    }

    pub fn isolated_copy_with_persona(
        &self,
        id: String,
        persistent: bool,
        persona: EffectivePersona,
    ) -> Self {
        self.try_isolated_copy_with_persona(id, persistent, persona)
            .expect("browser context persona conflicts with process identity")
    }

    pub fn try_isolated_copy_with_persona(
        &self,
        id: String,
        persistent: bool,
        persona: EffectivePersona,
    ) -> Result<Self, obscura_net::PersonaError> {
        obscura_net::activate_process_persona(&persona)?;
        let cookie_jar = Arc::new(if persistent {
            CookieJar::from_snapshot(&self.cookie_jar.snapshot())
        } else {
            CookieJar::new()
        });

        let mut client = ObscuraHttpClient::with_full_options(
            cookie_jar.clone(),
            self.proxy_url.as_deref(),
            self.allow_private_network,
        );
        client.block_trackers = true;
        if let Ok(mut guard) = client.user_agent.try_write() {
            *guard = persona.user_agent().to_string();
        }

        let storage_dir = persistent.then(|| self.storage_dir.clone()).flatten();
        let network_history = match storage_dir.as_deref() {
            Some(directory) => NetworkHistory::persistent(id.clone(), directory)
                .unwrap_or_else(|error| NetworkHistory::failed(id.clone(), error)),
            None => NetworkHistory::ephemeral(id.clone()),
        };
        let network_observation_root =
            obscura_js::network_observation::NetworkObservationQueue::default();
        if let Some(failure) = network_history.terminal_failure() {
            network_observation_root.fail_history(failure.message);
        }

        Ok(Self {
            id,
            cookie_jar,
            local_storage: Default::default(),
            http_client: Arc::new(client),
            persona: Arc::new(persona),
            proxy_url: self.proxy_url.clone(),
            robots_cache: Arc::new(RobotsCache::new()),
            obey_robots: self.obey_robots,
            allow_file_access: self.allow_file_access,
            storage_dir,
            allow_private_network: self.allow_private_network,
            network_history,
            network_observation_root: std::sync::Mutex::new(network_observation_root),
        })
    }

    pub fn save_cookies(&self) {
        if let Some(ref dir) = self.storage_dir {
            let _ = std::fs::create_dir_all(dir);
            let cookie_path = dir.join("cookies.json");
            if let Err(error) = self.cookie_jar.save_to_file(&cookie_path) {
                tracing::warn!("Failed to save cookies to {}: {}", cookie_path.display(), error);
            } else {
                tracing::info!("Saved cookies to {}", cookie_path.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obscura_net::StealthProfile;

    fn persona() -> EffectivePersona {
        EffectivePersona::builtin(StealthProfile::MacChrome153)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn context_uses_the_required_persona_before_first_request() {
        let ctx = BrowserContext::new("test".to_string(), persona());
        let client_ua = ctx.http_client.user_agent.read().await.clone();
        assert_eq!(ctx.persona().user_agent(), client_ua);
        assert!(ctx.http_client.block_trackers);
    }

    #[test]
    fn page_transport_is_built_from_the_frozen_context_persona() {
        let ctx = BrowserContext::new("persona".to_string(), persona());
        let page = crate::Page::new("persona-page".to_string(), Arc::new(ctx));
        let transport = page.stealth_client.transport_params();

        assert_eq!(transport.profile, StealthProfile::MacChrome153);
        assert_eq!(
            transport.accept_language.as_deref(),
            Some(page.context.persona().accept_language()),
        );
        assert_eq!(transport.do_not_track, None);
        assert_eq!(page.context.persona().user_agent(), transport.profile.user_agent());
    }

    #[test]
    fn checked_constructor_rejects_process_identity_conflicts() {
        let first = EffectivePersona::builtin(StealthProfile::WindowsChrome145);
        BrowserContext::try_new("first".to_string(), first).unwrap();

        let incompatible = EffectivePersona::builtin(StealthProfile::MacChrome153);
        let error = match BrowserContext::try_new("incompatible".to_string(), incompatible) {
            Ok(_) => panic!("a second process timezone and locale must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("process identity is already frozen"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn isolated_copy_preserves_persona_but_not_mutable_network_state() {
        let source = BrowserContext::new("source".to_string(), persona());
        source.cookie_jar.set_cookie(
            "sid=source",
            &url::Url::parse("https://example.com").unwrap(),
        );
        let persistent = source.isolated_copy("persistent".to_string(), true);
        let incognito = source.isolated_copy("incognito".to_string(), false);

        assert_eq!(persistent.persona().digest(), source.persona().digest());
        assert_eq!(incognito.persona().digest(), source.persona().digest());
        assert_eq!(persistent.cookie_jar.get_all_cookies().len(), 1);
        let subdomain = url::Url::parse("https://sub.example.com/").unwrap();
        assert!(persistent.cookie_jar.get_cookie_header(&subdomain).is_empty(),
            "a persistent context copy must retain host-only cookie scope");
        assert!(incognito.cookie_jar.get_all_cookies().is_empty());
        persistent.cookie_jar.clear();
        persistent.http_client.set_user_agent("Changed-UA/2.0").await;

        assert_eq!(source.cookie_jar.get_all_cookies().len(), 1);
        assert_eq!(
            source.http_client.user_agent.read().await.as_str(),
            source.persona().user_agent()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn page_registration_failure_stops_an_existing_context_runtime() {
        let mut context = BrowserContext::new("registration-limit".to_string(), persona());
        context.network_history = crate::network_history::NetworkHistory::with_limits(
            "registration-limit",
            crate::network_history::NetworkHistoryLimits {
                records: 1,
                ..Default::default()
            },
        );
        context.network_observation_root = std::sync::Mutex::new(
            obscura_js::network_observation::NetworkObservationQueue::default(),
        );
        let context = Arc::new(context);
        let mut first = crate::Page::new("first".to_string(), context.clone());
        first.navigate("data:text/html,first").await.unwrap();

        let second = crate::Page::new("second".to_string(), context.clone());

        assert!(second.network_history_page_instance_id().is_none());
        assert_eq!(
            context.network_history().terminal_failure().unwrap().kind,
            crate::network_history::NetworkHistoryFailureKind::Count,
        );
        let failure = first.network_observation_failure()
            .expect("existing runtime receives registration terminal");
        assert!(failure.message.contains("pageInstances limit 1"));
    }
}
