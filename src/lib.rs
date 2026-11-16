//! Shared result/configuration models, plugin catalogs and process adapters.
mod app_index;
mod calculator;
#[cfg(windows)]
pub mod clipboard_image;
pub mod config;
pub mod diagnostics;
pub mod drag_data;
mod file_io;
pub mod history;
pub mod hotkey;
#[cfg(windows)]
pub mod image_cache;
#[cfg(windows)]
pub mod input_attachment;
pub mod plugin_catalog;
pub mod plugin_config;
pub mod plugin_rpc;
pub mod plugin_trigger;
#[cfg(windows)]
pub mod plugins;
pub mod providers;
pub mod result;
pub mod scope_menu;
pub mod search;
pub mod storage;
pub mod web_search;
