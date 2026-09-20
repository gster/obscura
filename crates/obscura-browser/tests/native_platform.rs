#![cfg(feature = "render")]

#[path = "native_platform/support.rs"]
mod support;

#[path = "native_platform/controls.rs"]
mod controls;
#[path = "native_platform/focus.rs"]
mod focus;
#[path = "native_platform/forms.rs"]
mod forms;
#[path = "native_platform/history.rs"]
mod history;
#[path = "native_platform/location.rs"]
mod location;
#[path = "native_platform/persona_network.rs"]
mod persona_network;
#[path = "native_platform/text_input.rs"]
mod text_input;
#[path = "native_platform/web_platform.rs"]
mod web_platform;
