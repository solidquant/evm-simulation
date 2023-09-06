use anyhow::Result;
use cfmms::dex::DexVariant;
use ethers::providers::{Middleware, Provider, Ws};
use ethers::types::BlockNumber;
use log::info;
use std::sync::Arc;
use tokio::sync::broadcast::{self, Sender};
use tokio::task::JoinSet;

use evm_simulation::constants::Env;
use evm_simulation::honeypot::HoneypotFilter;
use evm_simulation::pools::{get_tokens, load_all_pools};
use evm_simulation::sandwich::sandwich_event_handler;
use evm_simulation::streams::{stream_new_blocks, stream_pending_transactions, Event};
use evm_simulation::utils::setup_logger;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    setup_logger()?;

    info!("[⚡️🦀⚡️ Starting EVM simulation]");

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
    honeypot_filter
        .filter_tokens(&pools[0..1000].to_vec())
        .await;

    let (event_sender, _): (Sender<Event>, _) = broadcast::channel(512);

    let mut set = JoinSet::new();

    set.spawn(stream_new_blocks(provider.clone(), event_sender.clone()));
    set.spawn(stream_pending_transactions(
        provider.clone(),
        event_sender.clone(),
    ));
    set.spawn(sandwich_event_handler(
        provider.clone(),
        event_sender.clone(),
    ));

    while let Some(res) = set.join_next().await {
        info!("{:?}", res);
    }

    Ok(())
}
