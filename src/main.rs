//! Dynamic (subprocess) entrypoint for the sabnzbd plugin.
//!
//! Serves this plugin over the orca socket via the typed `Plugin` builder.
//! The plugin is a `[[bin]]`, owns no runtime, and reaches orca only through
//! the socket. Advertises a single `service` backend.
plugin_toolkit::instrument::bootstrap!();
use plugin_toolkit::plugin::Plugin;
use sabnzbd::SabnzbdBackend;

fn main() -> plugin_toolkit::anyhow::Result<()> {
    Plugin::named("sabnzbd")
        .version(env!("CARGO_PKG_VERSION"))
        .service(SabnzbdBackend::new("sabnzbd"))
        .serve()
}
