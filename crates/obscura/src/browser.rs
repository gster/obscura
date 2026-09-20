use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use obscura_browser::BrowserContext;
use obscura_net::CookieJar;

use crate::config::BrowserConfig;
use crate::cookie::CookieStore;
use crate::error::Error;
use crate::page::Page;

static NEXT_PAGE_ID: AtomicU64 = AtomicU64::new(1);

pub struct Browser {
    context: Arc<BrowserContext>,
    cookie_jar: Arc<CookieJar>,
}

impl Browser {
    pub fn new(persona: obscura_net::EffectivePersona) -> Result<Self, Error> {
        Self::build(BrowserConfig::new(persona))
    }

    pub fn build(config: BrowserConfig) -> Result<Self, Error> {
        obscura_net::activate_process_persona(&config.persona)
            .map_err(anyhow::Error::from)?;
        let context = BrowserContext::with_options(
            "api".to_string(),
            config.persona,
            obscura_browser::BrowserContextOptions {
                proxy_url: config.proxy,
                storage_dir: config.storage_dir,
                ..Default::default()
            },
        );

        let context = Arc::new(context);
        let cookie_jar = context.cookie_jar.clone();

        Ok(Browser { context, cookie_jar })
    }

    pub fn builder(persona: obscura_net::EffectivePersona) -> BrowserBuilder {
        BrowserBuilder { config: BrowserConfig::new(persona) }
    }

    pub async fn new_page(&self) -> Result<Page, Error> {
        let id = NEXT_PAGE_ID.fetch_add(1, Ordering::Relaxed);
        let page = obscura_browser::Page::new(
            format!("page-{}", id),
            self.context.clone(),
        );
        Ok(Page {
            inner: RefCell::new(page),
        })
    }

    /// Access the cookie store for this browser session.
    pub fn cookies(&self) -> CookieStore {
        CookieStore::new(self.cookie_jar.clone())
    }
}

pub struct BrowserBuilder {
    config: BrowserConfig,
}

impl BrowserBuilder {
    pub fn proxy(mut self, proxy: impl Into<String>) -> Self {
        self.config.proxy = Some(proxy.into());
        self
    }
    pub fn storage_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.config.storage_dir = Some(dir.into());
        self
    }
    pub fn build(self) -> Result<Browser, Error> {
        Browser::build(self.config)
    }
}
