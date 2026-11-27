mod auth;
pub mod connection;
pub mod connector;
mod messages;

pub use connection::VncClient;
pub use connector::VncConnector;

#[cfg_attr(not(feature = "desktop-resize"), path = "desktop_disabled.rs")]
pub(crate) mod desktop;
