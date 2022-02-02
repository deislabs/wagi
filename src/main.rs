use wagi::{wagi_app, wagi_server::WagiServer};

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let startup_span = tracing::info_span!("total startup").entered();

    let configuration = wagi_app::parse_command_line()?;

    // TODO: this can all go into lib.rs as "build_routing_table"
    let handlers = wagi::handler_loader::load_handlers(&configuration).await?;
    // Possibly this should go into a 'routing table builder' so we cleanly separate
    // prep-time and serve-time responsibilities.
    let routing_table = wagi::dispatcher::RoutingTable::build(&handlers, configuration.request_global_context())?;

    let server = WagiServer::new(&configuration, routing_table).await?;

    drop(startup_span);

    println!("Ready: serving on {}", configuration.http_configuration.listen_on);
    server.serve().await
}
