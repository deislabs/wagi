mod wagi_app;

use wagi::{Router, wagi_config::required_blobs, wagi_server::WagiServer};

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let configuration = wagi_app::parse_command_line()?;
    let handlers = configuration.read_handler_configuration().await?;
    let required_blobs = required_blobs(&handlers).await?;

    // TODO: this is now done further down but it is the same code path each time - TBD
    // let routing_table = wagi::dispatcher::RoutingTable::build(&handlers);

    // validate the config content such as bindles and local module refs and parse it into useful form
    // prepare the ground (unless this should be lazy)
    // construct the dispatch map (is this the same thing as the validated config?)
    
    let router = Router::from_configuration(&configuration).await?;  // This does all three of the above!
    let server = WagiServer::new(&configuration, router).await?;

    println!("Ready: serving on {}", configuration.http_configuration.listen_on);
    server.serve().await
}
