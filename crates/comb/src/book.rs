//! What a hostname points at.
//!
//! The map, and the words for printing a site, kept apart from the proxy that
//! serves them. An engine has to carry the map wherever it runs, because a
//! project adds sites to whatever is hosting; it does not have to carry a TLS
//! stack to do that. This is the whole of what a build without `sites` keeps.

use std::collections::HashMap;
use std::sync::Arc;

/// What each hostname points at. Fixed for the life of a proxy: changing the
/// map means restarting it, which is what a config change already does.
pub type Sites = HashMap<String, u16>;

/// Shared, so a project can add its sites to a host that is already running
/// rather than needing one restarted at it.
pub type Book = Arc<std::sync::RwLock<Sites>>;

/// Where sites are served until a privileged helper can hand over 80 and 443.
/// A port in the url is the thing that milestone buys back.
pub const HTTPS_PORT: u16 = 8443;

/// The `:port` a url needs, which is nothing at all when it is the one browsers
/// already assume. Shared so the redirect and everything that prints a site
/// agree about when a port is worth showing.
pub fn port_suffix(port: u16) -> String {
    if port == 443 {
        String::new()
    } else {
        format!(":{port}")
    }
}

/// A site's url as a person should see it.
pub fn site_url(host: &str, port: u16) -> String {
    format!("https://{host}{}", port_suffix(port))
}

/// Where a bare hostname lands before it is sent on to https.
pub const HTTP_PORT: u16 = 8080;
