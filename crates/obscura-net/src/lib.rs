pub mod client;
pub mod response_body;
pub mod request_body;
pub mod observation;
pub mod persona;
pub mod headers;
pub use headers::{HeaderCapture, RawHeader};
pub mod cookies;
pub mod encoding;
pub mod interceptor;
pub mod robots;
pub mod blocklist;
pub mod network_activity;
pub mod stealth_client;

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
pub use network_activity::{NetworkActivityGuard, NetworkActivitySnapshot, NetworkActivityTracker};
pub use stealth_client::{
    StealthHttpClient, StealthProfile, STEALTH_NAVIGATOR_PLATFORM, STEALTH_UA_PLATFORM,
    STEALTH_UA_PLATFORM_VERSION, STEALTH_USER_AGENT,
};
pub use persona::{
    activate_process_persona, EffectivePersona, GeolocationSpec, PersonaError, PersonaSpec, ViewportSpec,
    PERSONA_SCHEMA_VERSION,
};
