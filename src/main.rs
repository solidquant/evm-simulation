use anyhow::Result;
use cfmms::dex::DexVariant;
use ethers::providers::{Http, Middleware, Provider, Ws};
use ethers::types::{BlockId, BlockNumber, H160, U256, U64};
use log::info;
use std::{collections::HashMap, str::FromStr, sync::Arc};

use evm_simulation::constants::Env;
use evm_simulation::honeypot::HoneypotFilter;
use evm_simulation::pools::{get_tokens, load_all_pools};
use evm_simulation::simulator::EvmSimulator;
use evm_simulation::tokens::{get_implementation, get_token_info};
use evm_simulation::trace::EvmTracer;
use evm_simulation::utils::setup_logger;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    setup_logger()?;

    info!("Starting EVM simulation");

    let env = Env::new();
    let factories = vec![(
        // Sushiswap V2
        "0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2Ac",
        DexVariant::UniswapV2,
        10794229u64,
    )];
    let pools = load_all_pools(env.wss_url.clone(), factories).await?;
    let tokens = get_tokens(&pools);
    info!("{:?}", tokens.len());

    let ws = Ws::connect(&env.wss_url).await.unwrap();
    let provider = Arc::new(Provider::new(ws));
    let block = provider
        .get_block(BlockNumber::Latest)
        .await
        .unwrap()
        .unwrap();

    let mut honeypot_filter = HoneypotFilter::new(provider.clone(), block.clone());
    honeypot_filter.setup().await;
    honeypot_filter.filter_tokens(&pools[0..100].to_vec()).await;

    Ok(())
}
