// Simple HTTPS echo service based on hyper-rustls, borrowed and modified from
// https://github.com/rustls/hyper-rustls/blob/3f16ac4c36d1133883073b7d6eacf8c09339e87f/examples/server.rs
//
// Note: We may want to make this our own little crate we share with the world if we end up using
// this in more than one place. As far as I can tell, there are no "TlsAcceptor" crates available
// out there
use core::task::{Context, Poll};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::vec::Vec;
use std::{fs, io, sync::Arc};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio_rustls::rustls::internal::pemfile;
use tokio_rustls::rustls::{self, ServerConfig};
use tokio_rustls::server::TlsStream;
use tokio_rustls::{Accept, TlsAcceptor};

fn error(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

pub(crate) struct TlsHyperAcceptor {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    in_progress_stream: Option<Accept<TcpStream>>,
}

impl TlsHyperAcceptor {
    pub(crate) async fn new(
        addr: impl ToSocketAddrs,
        cert_file: impl AsRef<Path>,
        key_file: impl AsRef<Path>,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let tls_cfg = {
            // Load public certificate.
            let certs = load_certs(cert_file)?;
            // Load private key.
            let key = load_private_key(key_file)?;
            // Do not use client certificate authentication.
            let mut cfg = ServerConfig::new(rustls::NoClientAuth::new());
            // Select a certificate to use.
            cfg.set_single_cert(certs, key)
                .map_err(|e| error(format!("{}", e)))?;
            // Configure ALPN to accept HTTP/1.1 (and not http2 due to differences in header
            // requirements, namely the HOST header). If we want to add http2 in the future, we can
            // add `b"h2".to_vec()` to the list
            cfg.set_protocols(&[b"http/1.1".to_vec()]);
            Arc::new(cfg)
        };
        Ok(TlsHyperAcceptor {
            listener,
            acceptor: tls_cfg.into(),
            in_progress_stream: None,
        })
    }
}

impl hyper::server::accept::Accept for TlsHyperAcceptor {
    type Conn = TlsStream<TcpStream>;
    type Error = io::Error;

    fn poll_accept(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
        let mut accept = match self.in_progress_stream.take() {
            Some(s) => {
                log::trace!("TLS handshake currently in progress. Polling for current status");
                s
            }
            None => {
                log::trace!("No handshake in progress, checking for new connection");
                let socket = match Pin::new(&mut self.listener).poll_accept(cx) {
                    Poll::Ready(Ok((socket, _))) => socket,
                    Poll::Ready(Err(e)) => return Poll::Ready(Some(Err(e))),
                    Poll::Pending => return Poll::Pending,
                };
                self.acceptor.accept(socket)
            }
        };

        match Pin::new(&mut accept).poll(cx) {
            Poll::Ready(Ok(i)) => {
                log::trace!("TLS handshake complete, returning active connection");
                Poll::Ready(Some(Ok(i)))
            }
            // Based on my testing, it seems like when someone passes an invalid certificate or you
            // try to make an http request to this endpoint, the error is always invalid data.
            // Perhaps we can eventually just swallow all errors, but this seems to be the most
            // common one
            Poll::Ready(Err(e)) if matches!(e.kind(), std::io::ErrorKind::InvalidData) => {
                log::trace!("Got invalid https request: {:?}", e);
                // We are explicitly not setting the in_progress_stream because there is nothing
                // more we can do with this connection as it is invalid. Wake the task so it can
                // poll for a new connection
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Pending => {
                self.in_progress_stream = Some(accept);
                Poll::Pending
            }
        }
    }
}

// Load public certificate from file.
fn load_certs(filename: impl AsRef<Path>) -> io::Result<Vec<rustls::Certificate>> {
    // Open certificate file.
    let certfile = fs::File::open(&filename).map_err(|e| {
        error(format!(
            "failed to open {}: {}",
            filename.as_ref().display(),
            e
        ))
    })?;
    let mut reader = io::BufReader::new(certfile);

    // Load and return certificate.
    pemfile::certs(&mut reader).map_err(|_| error("failed to load certificate".into()))
}

// Load private key from file.
fn load_private_key(filename: impl AsRef<Path>) -> io::Result<rustls::PrivateKey> {
    // Open keyfile.
    let keyfile = fs::File::open(&filename).map_err(|e| {
        error(format!(
            "failed to open {}: {}",
            filename.as_ref().display(),
            e
        ))
    })?;
    let mut reader = io::BufReader::new(keyfile);

    // Load and return a single private key.
    let keys = pemfile::pkcs8_private_keys(&mut reader)
        .map_err(|_| error("failed to load private key".into()))?;
    if keys.len() != 1 {
        return Err(error("expected a single private key".into()));
    }
    Ok(keys[0].clone())
}
