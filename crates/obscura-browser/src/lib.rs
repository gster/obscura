pub mod context;
mod fork_virtual_url;
pub mod lifecycle;
pub mod network_history;
pub mod page;
#[cfg(feature = "render")]
pub mod pdf;

pub use context::{BrowserContext, BrowserContextOptions};
pub use obscura_js::ops::DeviceIdentity;
pub use lifecycle::{LifecycleState, WaitUntil};
pub use network_history::{
    HistoryBodyCandidate, HistoryBodyChunk, HistoryBodyKey, HistoryBodyKind,
    HistoryBodyRef, NetworkHistory, NetworkHistoryCandidate, NetworkHistoryError,
    NetworkHistoryFailureKind, NetworkHistoryId, NetworkHistoryLimits,
    NetworkHistoryPage, NetworkHistoryQuery, NetworkHistoryRecord,
    OwnedHeaderCapture, OwnedNetworkEvent, OwnedRawHeader, PageHistoryWriter,
    PageInstanceId,
};
pub use obscura_js::HTML_TO_MARKDOWN_JS;
pub use obscura_js::runtime::{
    KeyboardInput, KeyboardInputPhase, MouseInput, MouseInputPhase, WheelInput,
};
#[cfg(feature = "render")]
pub use obscura_js::{
    validate_capture_region, AnimationSample, AnimationSampleMode, AnimationSampleTime,
    CaptureError, CaptureRegion,
};
pub use obscura_dom::NodeId;
pub use page::{
    AutomationWait, AutomationWaitError, DocumentIdentity, NetworkEvent, NetworkEventPhase,
    NetworkIdleEvidence, NetworkIdleOutcome, Page, PageError,
};
#[cfg(feature = "render")]
pub use pdf::{RasterPdfError, RasterPdfOptions, RasterPdfPageRange};
// Re-exported so the embeddable `obscura` crate (which depends on obscura-browser,
// not obscura-js) can surface the interception channel types.
pub use obscura_js::ops::{InterceptResolution, InterceptedRequest};
