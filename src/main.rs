use hyper::service::{make_service_fn, service_fn};
use hyper::Server;
use wagi::route;

#[tokio::main]
pub async fn main() {
    println!("=> Starting server");
    let addr = ([127, 0, 0, 1], 3000).into();

    let mk_svc =
        make_service_fn(|_conn| async { Ok::<_, std::convert::Infallible>(service_fn(route)) });

    let srv = Server::bind(&addr).serve(mk_svc);

    if let Err(e) = srv.await {
        eprintln!("server error: {}", e);
    }
}
