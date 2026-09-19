use std::path::PathBuf;
use std::sync::Arc;

use obscura_net::{CookieJar, ObscuraHttpClient, RobotsCache};

pub struct BrowserContext {
    pub id: String,
    pub cookie_jar: Arc<CookieJar>,
    pub local_storage: obscura_js::ops::SharedWebStorage,
    pub http_client: Arc<ObscuraHttpClient>,
    pub user_agent: String,
    pub platform: String,
    pub ua_platform: String,
    pub ua_platform_version: String,
    pub language: String,
    pub languages: Vec<String>,
    pub accept_language: String,
    pub do_not_track: Option<String>,
    pub webgl_vendor: String,
    pub webgl_renderer: String,
    pub device_identity: Option<obscura_js::ops::DeviceIdentity>,
    pub proxy_url: Option<String>,
    pub robots_cache: Arc<RobotsCache>,
    pub obey_robots: bool,
    /// Kept for source compatibility. Stealth is an invariant and is always true.
    pub stealth: bool,
    pub stealth_profile: obscura_net::StealthProfile,
    /// When true, CDP-driven navigation to file:// URLs is permitted.
    /// Default is false: a remote CDP client cannot point the browser
    /// at /etc/shadow even if Obscura is running as a privileged user.
    /// Flip on via `obscura serve --allow-file-access` for legitimate
    /// local-HTML testing workflows. The CLI's own `obscura fetch
    /// file://...` path is unaffected because it does not go through
    /// the CDP server.
    pub allow_file_access: bool,
    pub storage_dir: Option<PathBuf>,
    /// When true, the http client allows fetching localhost / RFC1918 /
    /// link-local addresses. Set via `--allow-private-network` (issue #33).
    /// Independent of `allow_file_access` because they cover different threat
    /// models: file:// is a local file-system read, while private-network is
    /// the broader SSRF gate from issue #4.
    pub allow_private_network: bool,
}

impl BrowserContext {
    pub fn new(id: String) -> Self {
        Self::_new_inner(id, None, false, None, None, false, obscura_net::StealthProfile::default())
    }

    /// Create a BrowserContext with an optional storage directory.
    /// When `storage_dir` is set, cookies are automatically loaded from
    /// `{storage_dir}/cookies.json` on creation.
    pub fn with_storage(
        id: String,
        storage_dir: Option<PathBuf>,
    ) -> Self {
        Self::_new_inner(id, None, false, None, storage_dir, false, obscura_net::StealthProfile::default())
    }

    /// Create a BrowserContext with full options including storage_dir.
    pub fn with_storage_full(
        id: String,
        proxy_url: Option<String>,
        _stealth: bool,
        user_agent: Option<String>,
        storage_dir: Option<PathBuf>,
    ) -> Self {
        Self::_new_inner(id, proxy_url, true, user_agent, storage_dir, false, obscura_net::StealthProfile::default())
    }

    /// Variant that also accepts the `allow_private_network` opt-in. All
    /// pre-existing constructors default it to `false`; callers that want the
    /// CLI's `--allow-private-network` (issue #33) behaviour go through here.
    pub fn with_storage_and_network(
        id: String,
        proxy_url: Option<String>,
        _stealth: bool,
        user_agent: Option<String>,
        storage_dir: Option<PathBuf>,
        allow_private_network: bool,
    ) -> Self {
        Self::_new_inner(id, proxy_url, true, user_agent, storage_dir, allow_private_network, obscura_net::StealthProfile::default())
    }

    /// Construct a context from the calibrated persona profile that owns both
    /// the JavaScript identity and the primp transport preset.
    pub fn with_persona_profile(
        id: String,
        proxy_url: Option<String>,
        profile: obscura_net::StealthProfile,
        storage_dir: Option<PathBuf>,
        allow_private_network: bool,
    ) -> Self {
        Self::_new_inner(id, proxy_url, true, None, storage_dir, allow_private_network, profile)
    }

    fn _new_inner(
        id: String,
        proxy_url: Option<String>,
        _stealth: bool,
        user_agent: Option<String>,
        storage_dir: Option<PathBuf>,
        allow_private_network: bool,
        stealth_profile: obscura_net::StealthProfile,
    ) -> Self {
        let cookie_jar = Arc::new(CookieJar::new());

        // Restore cookies from disk if storage_dir is configured
        if let Some(ref dir) = storage_dir {
            let cookie_path = dir.join("cookies.json");
            if cookie_path.exists() {
                match cookie_jar.load_from_file(&cookie_path) {
                    Ok(n) if n > 0 => {
                        tracing::info!("Loaded {} cookies from {}", n, cookie_path.display());
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("Failed to load cookies from {}: {}", cookie_path.display(), e);
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
        let resolved_ua = stealth_profile.user_agent().to_string();
        if let Some(user_agent) = user_agent {
            assert_eq!(
                user_agent, resolved_ua,
                "custom User-Agent overrides are unsupported because network and JavaScript identity must use one calibrated browser persona",
            );
        }
        let (platform, ua_platform, ua_platform_version) = stealth_profile.platform();
        // Sync the http client's UA at construction so navigation requests pick it
        // up before any async setup runs. The lock has no other holders here, so
        // try_write always succeeds; we fall back silently if it ever fails.
        if let Ok(mut guard) = client.user_agent.try_write() {
            *guard = resolved_ua.clone();
        }
        let http_client = Arc::new(client);
        BrowserContext {
            id,
            cookie_jar,
            local_storage: Default::default(),
            http_client,
            user_agent: resolved_ua,
            platform: platform.to_string(),
            ua_platform: ua_platform.to_string(),
            ua_platform_version: ua_platform_version.to_string(),
            language: "en-US".into(),
            languages: vec!["en-US".into(), "en".into()],
            accept_language: "en-US,en;q=0.9".into(),
            do_not_track: None,
            webgl_vendor: "Google Inc. (NVIDIA)".into(),
            webgl_renderer: "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)".into(),
            device_identity: None,
            proxy_url,
            robots_cache: Arc::new(RobotsCache::new()),
            obey_robots: false,
            stealth: true,
            stealth_profile,
            allow_file_access: false,
            storage_dir,
            allow_private_network,
        }
    }

    pub fn with_options(id: String, proxy_url: Option<String>, _stealth: bool) -> Self {
        Self::with_full_options(id, proxy_url, true, None)
    }

    pub fn with_full_options(
        id: String,
        proxy_url: Option<String>,
        _stealth: bool,
        user_agent: Option<String>,
    ) -> Self {
        Self::_new_inner(id, proxy_url, true, user_agent, None, false, obscura_net::StealthProfile::default())
    }

    pub fn with_proxy(id: String, proxy_url: Option<String>) -> Self {
        Self::with_options(id, proxy_url, false)
    }

    /// Create a context with the same browser configuration but independent
    /// mutable network state. Persistent copies start with the template's
    /// current cookies; incognito copies start empty and never write to the
    /// template's storage directory.
    pub fn isolated_copy(&self, id: String, persistent: bool) -> Self {
        let cookie_jar = Arc::new(CookieJar::new());
        if persistent {
            cookie_jar.set_cookies_from_cdp(self.cookie_jar.get_all_cookies());
        }

        let mut client = ObscuraHttpClient::with_full_options(
            cookie_jar.clone(),
            self.proxy_url.as_deref(),
            self.allow_private_network,
        );
        client.block_trackers = true;
        if let Ok(mut guard) = client.user_agent.try_write() {
            *guard = self.user_agent.clone();
        }

        BrowserContext {
            id,
            cookie_jar,
            local_storage: Default::default(),
            http_client: Arc::new(client),
            user_agent: self.user_agent.clone(),
            platform: self.platform.clone(),
            ua_platform: self.ua_platform.clone(),
            ua_platform_version: self.ua_platform_version.clone(),
            language: self.language.clone(),
            languages: self.languages.clone(),
            accept_language: self.accept_language.clone(),
            do_not_track: self.do_not_track.clone(),
            webgl_vendor: self.webgl_vendor.clone(),
            webgl_renderer: self.webgl_renderer.clone(),
            device_identity: self.device_identity.clone(),
            proxy_url: self.proxy_url.clone(),
            robots_cache: Arc::new(RobotsCache::new()),
            obey_robots: self.obey_robots,
            stealth: true,
            stealth_profile: self.stealth_profile,
            allow_file_access: self.allow_file_access,
            storage_dir: persistent.then(|| self.storage_dir.clone()).flatten(),
            allow_private_network: self.allow_private_network,
        }
    }

    /// Persist cookies to disk if storage_dir is configured.
    /// Called during graceful shutdown.
    pub fn save_cookies(&self) {
        if let Some(ref dir) = self.storage_dir {
            let _ = std::fs::create_dir_all(dir);
            let cookie_path = dir.join("cookies.json");
            if let Err(e) = self.cookie_jar.save_to_file(&cookie_path) {
                tracing::warn!("Failed to save cookies to {}: {}", cookie_path.display(), e);
            } else {
                tracing::info!("Saved cookies to {}", cookie_path.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "custom User-Agent overrides are unsupported")]
    fn with_full_options_rejects_custom_user_agent() {
        let _ = BrowserContext::with_full_options(
            "test".to_string(),
            None,
            false,
            Some("Custom-UA/1.0".to_string()),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn with_full_options_falls_back_to_chrome_default() {
        let ctx = BrowserContext::with_full_options(
            "test".to_string(),
            None,
            false,
            None,
        );
        assert!(ctx.user_agent.contains("Chrome"));
        let client_ua = ctx.http_client.user_agent.read().await.clone();
        assert!(client_ua.contains("Chrome"));
        assert_eq!(ctx.user_agent, client_ua);
        assert_eq!(ctx.user_agent, ctx.stealth_profile.user_agent());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn with_options_keeps_default_user_agent() {
        let ctx = BrowserContext::with_options("test".to_string(), None, false);
        assert!(ctx.user_agent.contains("Chrome"));
        assert!(ctx.stealth, "legacy false cannot disable the browser baseline");
        assert!(ctx.http_client.block_trackers);
    }

    #[test]
    fn page_primp_transport_is_built_from_the_context_persona() {
        let mut ctx = BrowserContext::with_persona_profile(
            "persona".to_string(),
            None,
            obscura_net::StealthProfile::MacChrome153,
            None,
            false,
        );
        ctx.accept_language = "zh-CN,zh;q=0.9".to_string();
        ctx.do_not_track = Some("1".to_string());
        let page = crate::Page::new("persona-page".to_string(), Arc::new(ctx));
        let transport = page.stealth_client.transport_params();

        assert_eq!(transport.profile, obscura_net::StealthProfile::MacChrome153);
        assert_eq!(transport.accept_language.as_deref(), Some("zh-CN,zh;q=0.9"));
        assert_eq!(transport.do_not_track.as_deref(), Some("1"));
        assert_eq!(page.context.user_agent, transport.profile.user_agent());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn isolated_copy_does_not_share_mutable_network_state() {
        let mut source = BrowserContext::with_full_options(
            "source".to_string(),
            None,
            false,
            None,
        );
        source.device_identity = Some(obscura_js::ops::DeviceIdentity {
            seed: 123, hardware_concurrency: 8, device_memory: 8.0,
            screen_width: 1920, screen_height: 1080,
        });
        source.cookie_jar.set_cookie("sid=source", &url::Url::parse("https://example.com").unwrap());

        let persistent = source.isolated_copy("persistent".to_string(), true);
        let incognito = source.isolated_copy("incognito".to_string(), false);

        assert_eq!(serde_json::to_value(&persistent.device_identity).unwrap(), serde_json::to_value(&source.device_identity).unwrap());
        assert_eq!(serde_json::to_value(&incognito.device_identity).unwrap(), serde_json::to_value(&source.device_identity).unwrap());
        assert_eq!(persistent.cookie_jar.get_all_cookies().len(), 1);
        assert!(incognito.cookie_jar.get_all_cookies().is_empty());
        persistent.cookie_jar.clear();
        persistent.http_client.set_user_agent("Changed-UA/2.0").await;

        assert_eq!(source.cookie_jar.get_all_cookies().len(), 1);
        assert_eq!(source.http_client.user_agent.read().await.as_str(), source.stealth_profile.user_agent());
    }
}
