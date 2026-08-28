//! The proxy, end to end. A real app on a real port, a real client that trusts
//! only the local authority, and a real request over TLS. Anything less would
//! prove the parts work without proving they fit together.

mod support;

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use comb::{Authority, Paths, Sites};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use support::TestHome;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_rustls::TlsConnector;

/// An app that reports what reached it, which is how the forwarded headers get
/// checked rather than assumed.
async fn app() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let serving = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let service = service_fn(|request: Request<Incoming>| async move {
                    let mut seen = String::new();
                    for (name, value) in request.headers() {
                        seen.push_str(&format!("{name}: {}\n", value.to_str().unwrap_or("")));
                    }
                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(seen))))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (port, serving)
}

struct Running {
    port: u16,
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

async fn proxy(home: &TestHome, sites: Sites) -> (Running, Arc<Authority>) {
    let authority = Arc::new(Authority::open(&Paths::new(home.path())).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopped) = oneshot::channel();

    let serving = authority.clone();
    tokio::spawn(async move {
        let _ = comb::serve_sites(listener, Arc::new(sites.into()), serving, async {
            let _ = stopped.await;
        })
        .await;
    });

    (
        Running {
            port,
            stop: Some(stop),
        },
        authority,
    )
}

/// A client that trusts the local authority and nothing else, which is exactly
/// the position a browser is in once the root is trusted.
async fn get(authority: &Authority, port: u16, host: &str) -> (StatusCode, String) {
    let mut roots = RootCertStore::empty();
    for certificate in CertificateDer::pem_file_iter(authority.root_file()).unwrap() {
        roots.add(certificate.unwrap()).unwrap();
    }

    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();

    let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let name = ServerName::try_from(host.to_string()).unwrap();
    let tls = TlsConnector::from(Arc::new(config))
        .connect(name, stream)
        .await
        .expect("the client should accept a certificate from the local authority");

    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(connection);

    let request = Request::builder()
        .uri("/")
        .header("host", host)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let response = sender.send_request(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn a_site_is_served_over_https_and_told_it_is_behind_one() {
    let home = TestHome::new();
    let (app_port, _app) = app().await;
    let sites: Sites = HashMap::from([("myapp.test".to_string(), app_port)]);
    let (running, authority) = proxy(&home, sites).await;

    let (status, seen) = get(&authority, running.port, "myapp.test").await;

    assert_eq!(status, StatusCode::OK, "{seen}");
    // The whole reason for speaking http rather than splicing bytes.
    assert!(seen.contains("x-forwarded-proto: https"), "{seen}");
    assert!(seen.contains("x-forwarded-host: myapp.test"), "{seen}");
    assert!(seen.contains("x-forwarded-for: 127.0.0.1"), "{seen}");
    assert!(
        seen.contains(&format!("x-forwarded-port: {app_port}")),
        "{seen}"
    );
}

#[tokio::test]
async fn a_site_whose_app_is_not_running_says_so() {
    let home = TestHome::new();
    // A port nothing is listening on: bound, read, then dropped.
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = dead.local_addr().unwrap().port();
    drop(dead);

    let sites: Sites = HashMap::from([("myapp.test".to_string(), port)]);
    let (running, authority) = proxy(&home, sites).await;

    let (status, body) = get(&authority, running.port, "myapp.test").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body.contains("nothing is listening"), "{body}");
    assert!(body.contains(&port.to_string()), "{body}");
}

#[tokio::test]
async fn a_name_we_do_not_serve_gets_no_certificate() {
    let home = TestHome::new();
    let (app_port, _app) = app().await;
    let sites: Sites = HashMap::from([("myapp.test".to_string(), app_port)]);
    let (running, authority) = proxy(&home, sites).await;

    // The authority must not mint a certificate for any name that is asked
    // for, or it becomes an oracle for signing arbitrary hosts.
    let mut roots = RootCertStore::empty();
    for certificate in CertificateDer::pem_file_iter(authority.root_file()).unwrap() {
        roots.add(certificate.unwrap()).unwrap();
    }
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    let stream = TcpStream::connect(("127.0.0.1", running.port))
        .await
        .unwrap();
    let name = ServerName::try_from("elsewhere.test".to_string()).unwrap();
    let handshake = TlsConnector::from(Arc::new(config))
        .connect(name, stream)
        .await;

    assert!(
        handshake.is_err(),
        "a name skep does not serve should not get a certificate"
    );
    // Nothing was written for it either.
    assert!(
        !home
            .path()
            .join("ca")
            .join("hosts")
            .join("elsewhere.test.pem")
            .exists()
    );
}

#[tokio::test]
async fn plain_http_is_sent_to_https() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopped) = oneshot::channel();
    tokio::spawn(async move {
        let _ = comb::redirect(listener, 8443, async {
            let _ = stopped.await;
        })
        .await;
    });

    let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(connection);

    let request = Request::builder()
        .uri("/dashboard?page=2")
        .header("host", "myapp.test")
        .body(Empty::<Bytes>::new())
        .unwrap();
    let response = sender.send_request(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "https://myapp.test:8443/dashboard?page=2",
        "the path and query have to survive the redirect"
    );
    let _ = stop.send(());
}

/// An app that upgrades and then echoes, which is the shape of a websocket
/// without the framing.
async fn upgrading_app() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let service = service_fn(|mut request: Request<Incoming>| async move {
                    let upgrading = hyper::upgrade::on(&mut request);
                    tokio::spawn(async move {
                        if let Ok(io) = upgrading.await {
                            let mut io = TokioIo::new(io);
                            let (mut reading, mut writing) = tokio::io::split(&mut io);
                            let _ = tokio::io::copy(&mut reading, &mut writing).await;
                        }
                    });
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(StatusCode::SWITCHING_PROTOCOLS)
                            .header(hyper::header::CONNECTION, "upgrade")
                            .header(hyper::header::UPGRADE, "echo")
                            .body(Full::new(Bytes::new()))
                            .unwrap(),
                    )
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades()
                    .await;
            });
        }
    });
    port
}

#[tokio::test]
async fn an_upgraded_connection_keeps_carrying_bytes() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let home = TestHome::new();
    let app_port = upgrading_app().await;
    let sites: Sites = HashMap::from([("myapp.test".to_string(), app_port)]);
    let (running, authority) = proxy(&home, sites).await;

    let mut roots = RootCertStore::empty();
    for certificate in CertificateDer::pem_file_iter(authority.root_file()).unwrap() {
        roots.add(certificate.unwrap()).unwrap();
    }
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    let stream = TcpStream::connect(("127.0.0.1", running.port))
        .await
        .unwrap();
    let name = ServerName::try_from("myapp.test".to_string()).unwrap();
    let tls = TlsConnector::from(Arc::new(config))
        .connect(name, stream)
        .await
        .unwrap();

    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(connection.with_upgrades());

    let request = Request::builder()
        .uri("/socket")
        .header("host", "myapp.test")
        .header(hyper::header::CONNECTION, "upgrade")
        .header(hyper::header::UPGRADE, "echo")
        .body(Empty::<Bytes>::new())
        .unwrap();
    let mut response = sender.send_request(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

    // Past this point the proxy is no longer reading http, just moving bytes.
    let upgraded = hyper::upgrade::on(&mut response).await.unwrap();
    let mut io = TokioIo::new(upgraded);
    io.write_all(b"still here").await.unwrap();

    let mut back = [0u8; 10];
    io.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"still here");
}
