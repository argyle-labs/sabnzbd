//! Dynamic (subprocess) entrypoint for the sabnzbd plugin.
//!
//! Serves this plugin over the orca socket via the typed `Plugin` builder.
//! The plugin is a `[[bin]]`, owns no runtime, and reaches orca only through
//! the socket. Advertises a `service` backend and the `sabnzbd.` tools.
plugin_toolkit::instrument::bootstrap!();
use plugin_toolkit::plugin::Plugin;
use sabnzbd::SabnzbdBackend;

#[allow(unused_imports)]
use sabnzbd::tools as _;

fn main() -> plugin_toolkit::anyhow::Result<()> {
    Plugin::named("sabnzbd")
        .version(env!("CARGO_PKG_VERSION"))
        .service(SabnzbdBackend::new("sabnzbd"))
        .tools(["sabnzbd."])
        .serve()
}
