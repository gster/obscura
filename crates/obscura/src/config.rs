use std::path::PathBuf;

/// Configuration for launching a Browser instance.
pub struct BrowserConfig {
    /// Validated immutable browser identity.
    pub persona: obscura_net::EffectivePersona,
    /// Proxy URL (e.g., "socks5://127.0.0.1:1080")
    pub proxy: Option<String>,
    /// Directory for persistent cookie storage
    pub storage_dir: Option<PathBuf>,
}

impl BrowserConfig {
    pub fn new(persona: obscura_net::EffectivePersona) -> Self {
        Self { persona, proxy: None, storage_dir: None }
    }

    pub fn builder(persona: obscura_net::EffectivePersona) -> BrowserConfigBuilder {
        BrowserConfigBuilder { config: Self::new(persona) }
    }
}

pub struct BrowserConfigBuilder {
    config: BrowserConfig,
}

impl BrowserConfigBuilder {
    pub fn proxy(mut self, proxy: impl Into<String>) -> Self {
        self.config.proxy = Some(proxy.into());
        self
    }

    pub fn storage_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.config.storage_dir = Some(dir.into());
        self
    }

    pub fn build(self) -> BrowserConfig {
        self.config
    }
}
