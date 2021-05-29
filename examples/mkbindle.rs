///! Simple tool for creating an example bindle.
///!
///! This uses the files `invoice.toml` and `hello.wasm` to create a bindle
///! named example.com/hello/1.0.0
///!
///! Note that if you change either file there, you might need to recheck the
///! size and SHA256 sum of the hello.wasm. Use `shasum -a 256 hello.wasm`
use sha2::Digest;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // Start with this invoice
    let data = std::fs::read_to_string("examples/invoice.toml")?;
    let inv: bindle::Invoice = toml::from_str(data.as_str())?;

    // Connect to our Bindle server
    let bindler = bindle::client::Client::new("http://localhost:8080/v1")?;

    // Create the invoice
    let icr = bindler.create_invoice(inv).await?;

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

    println!("You can now use example.com/hello/1.0.0");

    Ok(())
}

// This basically replicates shasum -a 256 examples/hello.wasm
/*
async fn hash_file() {
    let mut file = tokio::fs::File::open("examples/hello.wasm")
        .await
        .expect("file cannot be opened");
    let mut hasher = bindle::async_util::AsyncSha256::new();
    tokio::io::copy(&mut file, &mut hasher)
        .await
        .expect("hashing file failed");
    let sha = format!("{:x}", hasher.into_inner().unwrap().finalize());
    print!("Hash was {}", sha);
}
*/
