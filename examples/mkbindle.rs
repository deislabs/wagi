///! Simple tool for creating an example bindle.
///!
///! This uses the files `invoice.toml` and `hello.wasm` to create a bindle
///! named example.com/hello/1.0.0
///!
///! Note that if you change either file there, you might need to recheck the
///! size and SHA256 sum of the hello.wasm. Use `shasum -a 256 hello.wasm`

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // Start with the invoice in the examples directory
    let data = std::fs::read_to_string("examples/invoice.toml")?;
    let inv: bindle::Invoice = toml::from_str(data.as_str())?;
    let bindle_server =
        std::env::var("BINDLE_URL").unwrap_or_else(|_| "http://127.0.0.1:8080/v1".to_owned());

    // Connect to our Bindle server
    let bindler = bindle::client::Client::new(bindle_server.as_str())?;

    // Create the invoice
    let iid = inv.bindle.id.clone();
    let icr = bindler
        .create_invoice(inv)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create invoice for {}: {}", iid, e))?;

    // For every missing item, look for a local parcel and upload it.
    if let Some(items) = icr.missing {
        for item in items {
            bindler
                .create_parcel_from_file(
                    icr.invoice.bindle.id.clone(),
                    item.sha256.as_str(),
                    item.name,
                )
                .await?;
        }
    }

    println!("You can now use {}", iid);

    Ok(())
}
