//! Interactive symbol graph in the style of Sourcetrail: a structured
//! "trail" around the active symbol, or a per-file overview.

mod camera;
mod overview;
mod render;
mod scene;
mod trail;
mod view;

pub use view::GraphView;
