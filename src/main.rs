// SUP-006: no unsafe Rust in this crate; all unsafe comes from audited deps.
#![forbid(unsafe_code)]

#[tokio::main]
async fn main() {
    if let Err(error) = elasticdump_rs::run().await {
        let args: Vec<String> = std::env::args().collect();
        let msg = elasticdump_rs::endpoint::scrub_args(&format!("{error:#}"), &args);
        eprintln!("Error: {msg}");
        std::process::exit(elasticdump_rs::exit_code(&error));
    }
}
