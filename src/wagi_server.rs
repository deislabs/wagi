use std::net::SocketAddr;

use crate::{tls, wagi_config::TlsConfiguration};
use crate::wagi_config::WagiConfiguration;
use crate::Router;

use hyper::{
    server::conn::AddrStream,
    service::{make_service_fn, service_fn},
};
use hyper::{Body, Response, Server};
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;

pub struct WagiServer {
    router: Router,
    tls: Option<TlsConfiguration>,
    address: SocketAddr,
}

impl WagiServer {
    pub async fn new(configuration: &WagiConfiguration, router: Router) -> anyhow::Result<Self> {
        Ok(Self {
            router,
            tls: configuration.http_configuration.tls.clone(),
            address: configuration.http_configuration.listen_on.clone(),
        })
    }

    pub async fn serve(&self) -> anyhow::Result<()> {
        // NOTE(thomastaylor312): I apologize for the duplicated code here. I tried to work around this
        // by creating a GetRemoteAddr trait, but you can't use an impl Trait in a closure. The return
        // types for the service fns aren't exported and so I couldn't do a wrapper around the router
        // either. This means these services are basically the same, but with different connection types
        match &self.tls {
            Some(tls) => {
                let mk_svc = make_service_fn(move |conn: &TlsStream<TcpStream>| {
                    let (inner, _) = conn.get_ref();
                    // We are mapping the error because the normal error types are not cloneable and
                    // service functions do not like captured vars, even when moved
                    let addr_res = inner.peer_addr().map_err(|e| e.to_string());
                    let r = self.router.clone();
                    Box::pin(async move {
                        Ok::<_, std::convert::Infallible>(service_fn(move |req| {
                            let r2 = r.clone();
                            // NOTE: There isn't much in the way of error handling we can do here as
                            // this function needs to return an infallible future. Based on the
                            // documentation of the underlying getpeername function
                            // (https://man7.org/linux/man-pages/man2/getpeername.2.html and
                            // https://docs.microsoft.com/en-us/windows/win32/api/winsock/nf-winsock-getpeername)
                            // the only error that will probably occur here is an interrupted connection
                            let a_res = addr_res.clone();
                            async move {
                                match a_res {
                                    Ok(addr) => r2.route(req, addr).await,
                                    Err(e) => {
                                        tracing::error!(error = %e, "Socket connection error on new connection");
                                        Ok(Response::builder()
                                            .status(hyper::http::StatusCode::INTERNAL_SERVER_ERROR)
                                            .body(Body::from("Socket connection error"))
                                            .unwrap())
                                    }
                                }
                            }
                        }))
                    })
                });
                Server::builder(tls::TlsHyperAcceptor::new(&self.address, &tls.cert_path, &tls.key_path).await?)
                    .serve(mk_svc)
                    .await?;
            },
            None => {
                let mk_svc = make_service_fn(move |conn: &AddrStream| {
                    let addr = conn.remote_addr();
                    let r = self.router.clone();
                    async move {
                        Ok::<_, std::convert::Infallible>(service_fn(move |req| {
                            let r2 = r.clone();
                            async move { r2.route(req, addr).await }
                        }))
                    }
                });
                Server::bind(&self.address).serve(mk_svc).await?;
            },
        }
    
        Ok(())
    }
}
