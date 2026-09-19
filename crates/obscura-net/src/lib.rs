pub mod client;
pub mod cookies;
pub mod encoding;
pub mod interceptor;
pub mod robots;
pub mod blocklist;
pub mod stealth_client;
// Preserve the public module path for existing Rust callers.
pub use stealth_client as wreq_client;

pub use client::{
    env_allows_private_network, is_forbidden_ip, CallbackRegistry, ObscuraHttpClient,
    ObscuraNetError, RequestCallback, RequestCredentials, RequestInfo, RequestMode,
    ReferrerPolicy, ResourceRequest, ResourceType, Response, ResponseCallback, SsrfGuardResolver,
};
pub use cookies::{canonical_domain, default_cookie_path, CookieInfo, CookieJar};
pub use encoding::{
    decode_non_html, decode_response, decode_response_with_name, decode_with_label, label_name,
    url_encode_query,
};
pub use robots::RobotsCache;
pub use blocklist::is_blocked as is_tracker_blocked;
pub use stealth_client::{
    StealthHttpClient, StealthProfile, STEALTH_NAVIGATOR_PLATFORM, STEALTH_UA_PLATFORM,
    STEALTH_UA_PLATFORM_VERSION, STEALTH_USER_AGENT,
};
