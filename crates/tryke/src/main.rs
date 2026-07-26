#[tokio::main]
async fn main() -> anyhow::Result<tryke::ExitStatus> {
    tryke::run().await
}
