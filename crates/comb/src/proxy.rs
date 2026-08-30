//! A TLS terminating reverse proxy for local domains. Skep does not run your
//! app: it gives an app you are already running on a port a hostname and a
//! certificate browsers accept.
//!
//! Terminating and then splicing bytes would be simpler, but a spliced
//! connection cannot carry `x-forwarded-proto`, so frameworks behind it decide
//! they are on plain http and generate the wrong urls. Speaking http properly
//! is the cost of getting that right.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderName, HeaderValue};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

use crate::certs::Authority;
use crate::error::{Error, Result};

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
pub const HTTP_PORT: u16 = 8080;

type Body = BoxBody<Bytes, hyper::Error>;

/// Serves https for every configured site until `shutdown` completes.
pub async fn serve(
    listener: TcpListener,
    sites: Book,
    authority: Arc<Authority>,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<()> {
    let resolver = Arc::new(ByName {
        authority,
        sites: sites.clone(),
        issued: Mutex::new(HashMap::new()),
    });

    // The provider is named rather than taken from process-wide state, so
    // nothing depends on who installed a default first.
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|error| Error::Certificate(error.to_string()))?
            .with_no_client_auth()
            .with_cert_resolver(resolver);
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let shutdown = std::pin::pin!(shutdown);
    let mut shutdown = shutdown;
    loop {
        let accepted = tokio::select! {
            _ = &mut shutdown => return Ok(()),
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, peer)) = accepted else {
            continue;
        };

        let acceptor = acceptor.clone();
        let sites = sites.clone();
        tokio::spawn(async move {
            let Ok(tls) = acceptor.accept(stream).await else {
                // A browser refusing our certificate lands here. Nothing to
                // say to it over a connection it has already given up on.
                return;
            };
            let service = service_fn(move |request| answer(request, sites.clone(), peer));
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(tls), service)
                .with_upgrades()
                .await;
        });
    }
}

/// Redirects plain http to https, so a person typing a bare hostname still
/// arrives somewhere sensible.
pub async fn redirect(
    listener: TcpListener,
    https_port: u16,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<()> {
    let shutdown = std::pin::pin!(shutdown);
    let mut shutdown = shutdown;
    loop {
        let accepted = tokio::select! {
            _ = &mut shutdown => return Ok(()),
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, _)) = accepted else { continue };
        tokio::spawn(async move {
            let service = service_fn(move |request: Request<Incoming>| async move {
                // The port belongs in the url until something owns 443, and
                // sending people to a port nothing serves is worse than ugly.
                let shown = port_suffix(https_port);
                let target = match host_of(&request) {
                    Some(host) => format!("https://{host}{shown}{}", path_of(&request)),
                    None => {
                        return Ok::<_, Infallible>(say(StatusCode::BAD_REQUEST, "no host header"));
                    }
                };
                let response = Response::builder()
                    .status(StatusCode::PERMANENT_REDIRECT)
                    .header(hyper::header::LOCATION, target)
                    .body(empty())
                    .expect("a redirect is always well formed");
                Ok(response)
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

async fn answer(
    request: Request<Incoming>,
    sites: Book,
    peer: SocketAddr,
) -> std::result::Result<Response<Body>, Infallible> {
    let Some(host) = host_of(&request) else {
        return Ok(say(StatusCode::BAD_REQUEST, "no host header"));
    };
    let found = sites.read().ok().and_then(|book| book.get(&host).copied());
    let Some(port) = found else {
        return Ok(say(
            StatusCode::NOT_FOUND,
            &format!("{host} is not a site skep serves"),
        ));
    };

    Ok(forward(request, &host, port, peer).await)
}

async fn forward(
    mut request: Request<Incoming>,
    host: &str,
    port: u16,
    peer: SocketAddr,
) -> Response<Body> {
    // What the app behind us needs to know that the connection no longer says.
    set(&mut request, "x-forwarded-proto", "https");
    set(&mut request, "x-forwarded-host", host);
    set(&mut request, "x-forwarded-port", &port.to_string());
    set(&mut request, "x-forwarded-for", &peer.ip().to_string());

    let Ok(stream) = TcpStream::connect(("127.0.0.1", port)).await else {
        return say(
            StatusCode::BAD_GATEWAY,
            &format!("nothing is listening on port {port} for {host}"),
        );
    };

    let handshake = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await;
    let Ok((mut sender, connection)) = handshake else {
        return say(
            StatusCode::BAD_GATEWAY,
            "the app behind this site did not speak http",
        );
    };
    // Driven separately, and allowed to upgrade so websockets survive.
    tokio::spawn(connection.with_upgrades());

    // Taken before the request is handed over, because sending consumes it.
    let client_upgrade = hyper::upgrade::on(&mut request);

    let Ok(mut response) = sender.send_request(request).await else {
        return say(
            StatusCode::BAD_GATEWAY,
            "the app behind this site closed the connection",
        );
    };

    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let app_upgrade = hyper::upgrade::on(&mut response);
        tokio::spawn(async move {
            if let (Ok(browser), Ok(app)) = (client_upgrade.await, app_upgrade.await) {
                let mut browser = TokioIo::new(browser);
                let mut app = TokioIo::new(app);
                let _ = tokio::io::copy_bidirectional(&mut browser, &mut app).await;
            }
        });
    }

    response.map(|body| body.boxed())
}

/// Only sites we actually serve get a certificate. Minting one for whatever
/// name a client asks for would turn the authority into an oracle.
struct ByName {
    authority: Arc<Authority>,
    sites: Book,
    issued: Mutex<HashMap<String, Arc<CertifiedKey>>>,
}

// rustls asks for this; the authority and its keys have no business in a log.
impl std::fmt::Debug for ByName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ByName")
            .field(
                "sites",
                &self
                    .sites
                    .read()
                    .map(|book| book.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default(),
            )
            .finish()
    }
}

impl ResolvesServerCert for ByName {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name()?.to_string();
        if !self.sites.read().ok()?.contains_key(&name) {
            return None;
        }
        if let Some(ready) = self.issued.lock().ok()?.get(&name) {
            return Some(ready.clone());
        }

        let issued = self.authority.issue(&name).ok()?;
        let chain = CertificateDer::pem_slice_iter(issued.certificate_pem.as_bytes())
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()?;
        let key = PrivateKeyDer::from_pem_slice(issued.key_pem.as_bytes()).ok()?;
        let signing = rustls::crypto::ring::sign::any_supported_type(&key).ok()?;
        let ready = Arc::new(CertifiedKey::new(chain, signing));

        self.issued.lock().ok()?.insert(name, ready.clone());
        Some(ready)
    }
}

fn host_of<B>(request: &Request<B>) -> Option<String> {
    let raw = request
        .headers()
        .get(hyper::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| request.uri().host())?;
    // A host header may carry a port, which is never part of the site name.
    Some(raw.split(':').next()?.to_ascii_lowercase())
}

fn path_of<B>(request: &Request<B>) -> &str {
    request
        .uri()
        .path_and_query()
        .map(|both| both.as_str())
        .unwrap_or("/")
}

fn set<B>(request: &mut Request<B>, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        request
            .headers_mut()
            .insert(HeaderName::from_static(name), value);
    }
}

fn say(status: StatusCode, text: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(
            Full::new(Bytes::from(format!("{text}\n")))
                .map_err(|never| match never {})
                .boxed(),
        )
        .expect("a plain text response is always well formed")
}

fn empty() -> Body {
    Full::new(Bytes::new())
        .map_err(|never| match never {})
        .boxed()
}
