pub mod cdp_watchdog;
pub mod execution_cancellation;
pub mod frame;
mod import_map;
pub mod markdown;
pub mod module_loader;
pub mod network_observation;
pub mod ops;
pub mod runtime;
pub mod v8_flags;
pub mod worker;
mod worker_queue;
mod write_stream;

pub use markdown::HTML_TO_MARKDOWN_JS;
pub use v8_flags::{set_v8_flags, try_set_v8_flags, V8FlagsError, V8FlagsStatus};

// Screenshot rasterization (PNG bytes) from the render layer. Available when the
// render feature (which enables obscura-render/paint) is compiled in.
#[cfg(feature = "render")]
pub use obscura_render::{
    configure_font_directories, screenshot_png, screenshot_png_scrolled,
    screenshot_png_scrolled_at_animation_time,
    screenshot_png_scrolled_at_animation_time_with_surface_color,
    validate_capture_region, AnimationSample, AnimationSampleMode, AnimationSampleTime,
    CaptureError, CaptureRegion, CssMediaType, ImageRequestProfile,
    MAX_CAPTURE_DIMENSION, MAX_CAPTURE_PIXELS,
};
