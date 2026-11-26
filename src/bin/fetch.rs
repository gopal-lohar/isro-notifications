use std::path::PathBuf;

use isro_notifications::scrapper;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://www.isro.gov.in".into());
    let dir = PathBuf::from(std::env::var("CAHCE_DIR").unwrap_or_else(|_| "./".into()));

    let path = scrapper::fetch(&scrapper::client(), &url, &dir).await?;
    println!("{}", path.display());
    Ok(())
}
