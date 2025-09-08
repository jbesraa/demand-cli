#[tokio::main]
async fn main() {
    demand_cli::run_proxy().await;
}
