mod wagi_app;

use wagi::wagi_server::WagiServer;

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let configuration = wagi_app::parse_command_line()?;
    let server = WagiServer::new(configuration).await?;
    server.serve().await
}
