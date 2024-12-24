mod config;
mod token_service;

use config::Config;
use token_service::TokenService;
use tokio_postgres::NoTls;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load()?;
    let mint = "61V8vBaqAGMpgDQi4JcAwo1dmBGHsyhzodcPqnEVpump";
    let token_service = TokenService::new(&config.rpc_url, &config.db_url).await?;
    let metadata = token_service.get_metadata(mint).await?;
    println!("Metadata: {:?}", metadata);
    let price = token_service.get_price(mint).await?;
    println!("Price (cached): {:?}", price);
    Ok(())
}
