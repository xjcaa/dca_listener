use mpl_token_metadata::accounts::Metadata;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solana_client::rpc_client::RpcClient;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use spl_token::state::Mint;
use std::str::FromStr;
use tokio_postgres::{Client, NoTls};

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenMetadata {
    pub mint: String,
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub supply: u64,
}
pub struct TokenService {
    rpc_client: RpcClient,
    db_client: Client,
    http_client: reqwest::Client,
    price_cache_duration: u64, // seconds
}

impl TokenService {
    pub async fn new(
        rpc_url: &str,
        database_url: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        println!("Connecting to RPC...");
        let rpc_client = RpcClient::new(rpc_url.to_string());

        println!("Connecting to database: {}", database_url);
        let (db_client, connection) =
            tokio_postgres::connect(database_url, NoTls)
                .await
                .map_err(|e| {
                    eprintln!("Database connection error: {:?}", e);
                    e
                })?;

        // Spawn the connection handler
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("Connection error: {}", e);
            }
        });

        db_client
            .execute(
                "CREATE TABLE IF NOT EXISTS token_metadata (
                mint TEXT PRIMARY KEY,
                metadata JSONB NOT NULL,
                last_updated BIGINT NOT NULL
            )",
                &[],
            )
            .await?;

        // Create price cache table
        db_client
            .execute(
                "CREATE TABLE IF NOT EXISTS token_prices (
                mint TEXT PRIMARY KEY,
                price DOUBLE PRECISION NOT NULL,
                last_updated BIGINT NOT NULL
            )",
                &[],
            )
            .await?;

        Ok(Self {
            rpc_client,
            db_client,
            http_client: reqwest::Client::new(),
            price_cache_duration: 60,
        })
    }

    pub async fn get_metadata(
        &self,
        mint: &str,
    ) -> Result<TokenMetadata, Box<dyn std::error::Error>> {
        // Check cache first
        if let Some(metadata) = self.get_from_cache(mint).await? {
            return Ok(metadata);
        }

        // If not in cache or expired, fetch from chain
        let metadata = self.fetch_token_metadata(mint).await?;
        self.save_to_cache(&metadata).await?;

        Ok(metadata)
    }

    async fn get_from_cache(
        &self,
        mint: &str,
    ) -> Result<Option<TokenMetadata>, Box<dyn std::error::Error>> {
        let row = self
            .db_client
            .query_opt(
                "SELECT metadata FROM token_metadata WHERE mint = $1",
                &[&mint],
            )
            .await?;

        Ok(match row {
            Some(row) => Some(serde_json::from_value(row.get(0))?),
            None => None,
        })
    }

    async fn save_to_cache(
        &self,
        metadata: &TokenMetadata,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let json = serde_json::to_value(metadata)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        self.db_client
            .execute(
                "INSERT INTO token_metadata (mint, metadata, last_updated) 
                 VALUES ($1, $2, $3)
                 ON CONFLICT (mint) DO UPDATE SET metadata = $2, last_updated = $3",
                &[&metadata.mint, &json, &now],
            )
            .await?;
        Ok(())
    }

    async fn fetch_token_metadata(
        &self,
        mint: &str,
    ) -> Result<TokenMetadata, Box<dyn std::error::Error>> {
        let mint_pubkey = Pubkey::from_str(mint)?;
        let mint_account = self.rpc_client.get_account(&mint_pubkey)?;
        let mint_data = Mint::unpack(&mint_account.data)?;

        let (metadata_pda, _) = Pubkey::find_program_address(
            &[
                b"metadata",
                mpl_token_metadata::ID.as_ref(),
                mint_pubkey.as_ref(),
            ],
            &mpl_token_metadata::ID,
        );

        let metadata_account = self.rpc_client.get_account(&metadata_pda)?;
        let metadata = Metadata::from_bytes(&metadata_account.data)?;

        Ok(TokenMetadata {
            mint: mint.to_string(),
            name: metadata.name.trim_matches(char::from(0)).to_string(),
            symbol: metadata.symbol.trim_matches(char::from(0)).to_string(),
            decimals: mint_data.decimals,
            supply: mint_data.supply,
        })
    }

    pub async fn fetch_mint_price(&self, mint: &str) -> Result<f64, Box<dyn std::error::Error>> {
        let url = format!("https://api.jup.ag/price/v2?ids={}", mint);
        let response = self.http_client.get(&url).send().await?;
        let data: Value = response.json().await?;
        let price = f64::from_str(&data["data"][mint]["price"].as_str().unwrap_or("0.0"))?;
        Ok(price)
    }

    pub async fn get_price(&self, mint: &str) -> Result<f64, Box<dyn std::error::Error>> {
        // Check cache first
        if let Some(price) = self.get_price_from_cache(mint).await? {
            return Ok(price);
        }

        // If not in cache or expired, fetch from API
        let price = self.fetch_mint_price(mint).await?;
        self.save_price_to_cache(mint, price).await?;

        Ok(price)
    }

    async fn get_price_from_cache(
        &self,
        mint: &str,
    ) -> Result<Option<f64>, Box<dyn std::error::Error>> {
        let row = self
            .db_client
            .query_opt(
                "SELECT price, last_updated FROM token_prices 
                 WHERE mint = $1 AND last_updated > $2",
                &[
                    &mint,
                    &(std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs() as i64
                        - self.price_cache_duration as i64),
                ],
            )
            .await?;

        Ok(row.map(|row| row.get(0)))
    }

    async fn save_price_to_cache(
        &self,
        mint: &str,
        price: f64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        self.db_client
            .execute(
                "INSERT INTO token_prices (mint, price, last_updated) 
                 VALUES ($1, $2, $3)
                 ON CONFLICT (mint) DO UPDATE SET price = $2, last_updated = $3",
                &[&mint, &price, &now],
            )
            .await?;
        Ok(())
    }
}
