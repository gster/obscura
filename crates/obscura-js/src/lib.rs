mod audio;
mod speech_owner;
mod speech_protocol;
mod speech_provider;
mod speech_inventory;
mod speech_native_bindings;
mod speech_startup;
mod html_open_startup;
mod timing;
pub mod indexed_db;
pub mod cdp_watchdog;
pub mod csp;
pub mod execution_cancellation;
pub mod frame;
mod realm_factory;
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
#[cfg(test)]
mod browser_compat_tests;
#[cfg(test)]
mod todo_regressions;

pub use markdown::HTML_TO_MARKDOWN_JS;
pub use v8_flags::{set_v8_flags, try_set_v8_flags, V8FlagsError, V8FlagsStatus};

// Screenshot rasterization (PNG bytes) from the render layer. Available when the
// render feature (which enables obscura-render/paint) is compiled in.
#[cfg(feature = "render")]
pub use obscura_render::{
    screenshot_png, screenshot_png_scrolled,
    screenshot_png_scrolled_at_animation_time,
    screenshot_png_scrolled_at_animation_time_with_surface_color,
    validate_capture_region, AnimationSample, AnimationSampleMode, AnimationSampleTime,
    CaptureError, CaptureRegion, CssMediaType, ImageRequestProfile,
    MAX_CAPTURE_DIMENSION, MAX_CAPTURE_PIXELS,
};

pub use obscura_render::configure_font_directories;

#[cfg(test)]
mod speech_inventory_integration_tests;

pub mod pending_input;
mod scheduling;

#[cfg(test)]
mod realm_factory_tests;
