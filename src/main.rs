mod wagi_app;

use wagi::{Router, wagi_server::WagiServer};

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let configuration = wagi_app::parse_command_line()?;
    // validate the config content such as bindles and local module refs and parse it into useful form
    // prepare the ground (unless this should be lazy)
    // construct the dispatch map (is this the same thing as the validated config?)
    let router = Router::from_configuration(&configuration).await?;  // This does all three of the above!
    let server = WagiServer::new(&configuration, router).await?;
    server.serve().await
}
