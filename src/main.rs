mod wagi_app;

use wagi::{wagi_server::WagiServer};

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let configuration = wagi_app::parse_command_line()?;

    let emplacer = wagi::emplacer::Emplacer::new(&configuration).await?;
    emplacer.emplace_all().await?;

    let handlers = configuration.load_handler_configuration(&emplacer).await?;
    let routing_table = wagi::dispatcher::RoutingTable::build(&handlers, configuration.request_global_context())?;

    let server = WagiServer::new(&configuration, routing_table).await?;

    println!("Ready: serving on {}", configuration.http_configuration.listen_on);
    server.serve().await
}
