//! Central state management: data models, persistence, and the app state entity.

pub mod app_state;
pub mod models;
pub mod persistence;
pub mod sample_data;

pub use app_state::{AppEvent, AppState};
pub use models::*;
