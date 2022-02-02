use wagi::{wagi_app, wagi_server::WagiServer, handler_compiler::compile_all};

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let startup_span = tracing::info_span!("total startup").entered();

    let configuration = wagi_app::parse_command_line()?;

    let emplacer = wagi::emplacer::Emplacer::new(&configuration).await?;
    let pre_handler_config = emplacer.emplace_all().await?;

    let uncompiled_handlers = configuration.load_handler_configuration(pre_handler_config).await?;
    let handlers = compile_all(uncompiled_handlers, configuration.wasm_compilation_settings())?;
    let routing_table = wagi::dispatcher::RoutingTable::build(&handlers, configuration.request_global_context())?;

    let server = WagiServer::new(&configuration, routing_table).await?;

    drop(startup_span);

    println!("Ready: serving on {}", configuration.http_configuration.listen_on);
    server.serve().await
}
