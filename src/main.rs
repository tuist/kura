#[tokio::main]
async fn main() {
    if let Err(error) = cache::run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
