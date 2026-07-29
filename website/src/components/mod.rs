//! Reusable layout & UI components.

mod shell;
mod nav;
mod footer;
mod hero;
mod feature_card;
mod comparison_table;
pub mod pricing;
mod download_buttons;
mod reveal;
mod theme_toggle;

pub use shell::Shell;
pub use nav::Nav;
pub use footer::Footer;
pub use hero::Hero;
pub use feature_card::{FeatureCard, FeatureGroup};
pub use comparison_table::ComparisonTable;
pub use pricing::PricingCards;
pub use download_buttons::DownloadButtons;
pub use reveal::Reveal;
pub use theme_toggle::ThemeToggle;
