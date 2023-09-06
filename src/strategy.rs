use anvil::eth::fees::calculate_next_block_base_fee;
use anyhow::{anyhow, Result};
use cfmms::dex::DexVariant;
use colored::Colorize;
use ethers::{
    prelude::*,
    providers::{Middleware, Provider, Ws},
    types::{Address, BlockId, BlockNumber, H160, U256, U64},
};
use log::info;
use std::{collections::HashMap, str::FromStr, sync::Arc};
use tokio::sync::broadcast::Sender;

use crate::constants::Env;
use crate::honeypot::HoneypotFilter;
use crate::pools::{load_all_pools, Pool};
use crate::streams::{Event, NewBlock};

#[macro_export]
macro_rules! log_info_warning {
    ($($arg:tt)*) => {
        info!("{}", format_args!($($arg)*).to_string().magenta());
    };
}

pub async fn get_touched_pools<M: Middleware + 'static>(
    provider: Arc<Provider<Ws>>,
    tx: &Transaction,
    block_number: U64,
    verified_pools: &Vec<Pool>,
    honeypot_filter: &HoneypotFilter<M>,
) -> Result<()> {
    // you don't know what transaction will touch the pools you're interested in
    // thus, you need to trace all pending transactions you receive
    // evm tracing can sometimes take a very long time as can be seen from:
    // https://banteg.mirror.xyz/3dbuIlaHh30IPITWzfT1MFfSg6fxSssMqJ7TcjaWecM
    let verified_pools_address: Vec<H160> = verified_pools.into_iter().map(|p| p.address).collect();
    let trace = provider
        .debug_trace_call(
            tx,
            Some(BlockId::Number(BlockNumber::Number(block_number))),
            GethDebugTracingCallOptions {
                tracing_options: GethDebugTracingOptions {
                    disable_storage: None,
                    disable_stack: None,
                    enable_memory: None,
                    enable_return_data: None,
                    tracer: Some(GethDebugTracerType::BuiltInTracer(
                        GethDebugBuiltInTracerType::PreStateTracer,
                    )),
                    tracer_config: None,
                    timeout: None,
                },
                state_overrides: None,
            },
        )
        .await?;

    match trace {
        GethTrace::Known(known) => match known {
            GethTraceFrame::PreStateTracer(prestate) => match prestate {
                PreStateFrame::Default(prestate_mode) => {
                    let touched_accounts = prestate_mode.0;

                    // let touched_pools = Vec::new();

                    for (acc, acc_state) in &touched_accounts {
                        if verified_pools_address.contains(acc) {
                            match &acc_state.storage {
                                Some(state) => {
                                    info!("{:?}", state);
                                }
                                None => {}
                            }
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        },
        _ => {}
    }

    Ok(())
}

pub async fn event_handler(provider: Arc<Provider<Ws>>, event_sender: Sender<Event>) {
    let env = Env::new();
    let factories = vec![(
        // Sushiswap V2
        "0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2Ac",
        DexVariant::UniswapV2,
        10794229u64,
    )];
    let pools = load_all_pools(env.wss_url.clone(), factories)
        .await
        .unwrap();

    let block = provider
        .get_block(BlockNumber::Latest)
        .await
        .unwrap()
        .unwrap();

    let mut honeypot_filter = HoneypotFilter::new(provider.clone(), block.clone());
    honeypot_filter.setup().await;
    honeypot_filter
        .filter_tokens(&pools[0..3000].to_vec())
        .await;

    // filter out pools that use unverified tokens
    let verified_pools: Vec<Pool> = pools
        .into_iter()
        .filter(|pool| {
            let token0_verified = honeypot_filter.safe_token_info.contains_key(&pool.token0)
                || honeypot_filter.token_info.contains_key(&pool.token0);
            let token1_verified = honeypot_filter.safe_token_info.contains_key(&pool.token1)
                || honeypot_filter.token_info.contains_key(&pool.token1);
            token0_verified && token1_verified
        })
        .collect();
    info!("Verified pools only: {:?} pools", verified_pools.len());

    let mut event_receiver = event_sender.subscribe();

    let mut new_block = NewBlock {
        block_number: block.number.unwrap(),
        base_fee: block.base_fee_per_gas.unwrap_or_default(),
        next_base_fee: U256::from(calculate_next_block_base_fee(
            block.gas_used.as_u64(),
            block.gas_limit.as_u64(),
            block.base_fee_per_gas.unwrap_or_default().as_u64(),
        )),
    };

    loop {
        match event_receiver.recv().await {
            Ok(event) => match event {
                Event::Block(block) => {
                    new_block = block;
                }
                Event::PendingTx(tx) => {
                    let base_fee_condition =
                        tx.max_fee_per_gas.unwrap_or_default() < new_block.next_base_fee;

                    if base_fee_condition {
                        // log_info_warning!("Skipping {:?} mf < nbf", tx.hash);
                        continue;
                    }

                    // info!("Block #{:?}: {:?}", new_block.block_number, tx.hash);
                    match get_touched_pools(
                        provider.clone(),
                        &tx,
                        new_block.block_number,
                        &verified_pools,
                        &honeypot_filter,
                    )
                    .await
                    {
                        Ok(_) => {}
                        Err(_) => {}
                    }
                    // break;
                }
                Event::Log(_) => {}
            },
            Err(_) => {}
        }
    }
}
