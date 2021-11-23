use wagi::{wagi_app, wagi_server::WagiServer};

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let startup_span = tracing::info_span!("total startup").entered();

    let configuration = wagi_app::parse_command_line()?;

    let emplacer = wagi::emplacer::Emplacer::new(&configuration).await?;
    let pre_handler_config = emplacer.emplace_all().await?;

    let handlers = configuration.load_handler_configuration(pre_handler_config).await?;
    let routing_table = wagi::dispatcher::RoutingTable::build(&handlers, configuration.request_global_context())?;

    let server = WagiServer::new(&configuration, routing_table).await?;

    drop(startup_span);

    println!("Ready: serving on {}", configuration.http_configuration.listen_on);
    server.serve().await
}
