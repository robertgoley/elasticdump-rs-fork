#[tokio::main]
async fn main() {
    if let Err(error) = elasticdump_rs::run().await {
        eprintln!("Error: {error:#}");
        std::process::exit(elasticdump_rs::exit_code(&error));
    }
}
