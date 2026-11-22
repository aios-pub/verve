//! Sci-fi / instrumentation effects. Each component degrades gracefully on
//! touch and reduced-motion devices (see [`motion::effects_enabled`]).

mod boot_line;
mod count_up;
mod motion;
mod particle_field;
mod scroll_progress;
mod spotlight_card;
mod tech_badge;

pub use boot_line::BootLine;
pub use count_up::CountUp;
pub use motion::effects_enabled;
pub use particle_field::ParticleField;
pub use scroll_progress::ScrollProgress;
pub use spotlight_card::SpotlightCard;
pub use tech_badge::TechBadge;
