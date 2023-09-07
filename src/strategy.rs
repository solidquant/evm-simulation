use anvil::eth::fees::calculate_next_block_base_fee;
use anyhow::Result;
use cfmms::dex::DexVariant;
use colored::Colorize;
use ethers::{
    prelude::*,
    providers::{Middleware, Provider, Ws},
    types::{BlockId, BlockNumber, H160, U256, U64},
};
use foundry_evm::revm::primitives::keccak256;
use log::info;
use std::{collections::HashMap, str::FromStr, sync::Arc};
use tokio::sync::broadcast::Sender;

use crate::constants::Env;
use crate::honeypot::HoneypotFilter;
use crate::pools::{load_all_pools, Pool};
use crate::sandwich::{simulate_sandwich_bundle, Sandwich, SandwichSimulator};
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
    verified_pools_map: &HashMap<H160, Pool>,
    honeypot_filter: &HoneypotFilter<M>,
) -> Result<HashMap<H160, Option<H160>>> {
    // you don't know what transaction will touch the pools you're interested in
    // thus, you need to trace all pending transactions you receive
    // evm tracing can sometimes take a very long time as can be seen from:
    // https://banteg.mirror.xyz/3dbuIlaHh30IPITWzfT1MFfSg6fxSssMqJ7TcjaWecM

    // Also check: https://github.com/ethereum/go-ethereum/pull/25422#discussion_r978789901 for diffMode
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
                    tracer_config: Some(GethDebugTracerConfig::BuiltInTracer(
                        GethDebugBuiltInTracerConfig::PreStateTracer(PreStateConfig {
                            diff_mode: Some(true),
                        }),
                    )),
                    timeout: None,
                },
                state_overrides: None,
            },
        )
        .await?;

    let mut sandwichable_pools = HashMap::new();

    match trace {
        GethTrace::Known(known) => match known {
            GethTraceFrame::PreStateTracer(prestate) => match prestate {
                PreStateFrame::Diff(diff) => {
                    // Step 1: Check if any of the pools I'm monitoring were touched
                    let mut touched_pools = Vec::new();
                    for (acc, _) in &diff.post {
                        if verified_pools_map.contains_key(&acc) {
                            touched_pools.push(*acc);
                            sandwichable_pools.insert(*acc, None);
                        }
                    }

                    if touched_pools.is_empty() {
                        return Ok(sandwichable_pools);
                    }

                    let safe_token_info = &honeypot_filter.safe_token_info;
                    let balance_slots = &honeypot_filter.balance_slots;

                    // Step 2: Check if the transaction increases the pool's safe token balance (weth/usdt/usdc/dai)
                    // This means that the safe token price will go down, and the other token price will go up
                    // Thus, we buy the token in our frontrunning tx, and sell the token in our backrunning tx
                    for (_, safe_token) in safe_token_info {
                        let token_prestate = diff.pre.get(&safe_token.address);
                        match token_prestate {
                            Some(prestate) => match &prestate.storage {
                                Some(pre_storage) => {
                                    let slot = *balance_slots.get(&safe_token.address).unwrap();
                                    for pool in &touched_pools {
                                        let balance_slot = keccak256(&abi::encode(&[
                                            abi::Token::Address((*pool).into()),
                                            abi::Token::Uint(U256::from(slot)),
                                        ]));
                                        if pre_storage.contains_key(&balance_slot.into()) {
                                            let pre_balance = U256::from(
                                                pre_storage
                                                    .get(&balance_slot.into())
                                                    .unwrap()
                                                    .to_fixed_bytes(),
                                            );

                                            let token_poststate =
                                                diff.post.get(&safe_token.address).unwrap();
                                            let post_storage = &token_poststate.storage;
                                            let post_balance = U256::from(
                                                post_storage
                                                    .as_ref()
                                                    .unwrap()
                                                    .get(&balance_slot.into())
                                                    .unwrap()
                                                    .to_fixed_bytes(),
                                            );

                                            if pre_balance < post_balance {
                                                sandwichable_pools
                                                    .insert(*pool, Some(safe_token.address));
                                            }
                                        }
                                    }
                                }
                                None => {}
                            },
                            None => {}
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        },
        _ => {}
    }

    Ok(sandwichable_pools)
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

    let mut verified_pools_map = HashMap::new();
    for pool in &verified_pools {
        verified_pools_map.insert(pool.address, pool.clone());
    }

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
                    info!("⛓ New Block: {:?}", block);
                }
                Event::PendingTx(tx) => {
                    let base_fee_condition =
                        tx.max_fee_per_gas.unwrap_or_default() < new_block.base_fee;

                    if base_fee_condition {
                        continue;
                    }

                    match get_touched_pools(
                        provider.clone(),
                        &tx,
                        new_block.block_number,
                        &verified_pools_map,
                        &honeypot_filter,
                    )
                    .await
                    {
                        Ok(touched_pools) => {
                            if touched_pools.len() > 0 {
                                info!(
                                    "[🌯🥪🌯🥪🌯] Sandwichable pools detected: {:?}",
                                    touched_pools
                                );

                                let owner =
                                    H160::from_str("0x001a06BF8cE4afdb3f5618f6bafe35e9Fc09F187")
                                        .unwrap();

                                for (touched_pool, use_token) in &touched_pools {
                                    match use_token {
                                        Some(safe_token) => {
                                            let target_token = honeypot_filter
                                                .safe_token_info
                                                .get(safe_token)
                                                .unwrap();
                                            let target_pool =
                                                verified_pools_map.get(touched_pool).unwrap();
                                            let balance_slot = honeypot_filter
                                                .balance_slots
                                                .get(safe_token)
                                                .unwrap();
                                            let amount_in = U256::from(1)
                                                .checked_mul(
                                                    U256::from(10)
                                                        .pow(U256::from(target_token.decimals)),
                                                )
                                                .unwrap();

                                            let sandwich = Sandwich {
                                                amount_in,
                                                balance_slot: *balance_slot,
                                                target_token: target_token.clone(),
                                                target_pool: target_pool.clone(),
                                                meat_tx: tx.clone(),
                                            };

                                            match simulate_sandwich_bundle(
                                                sandwich,
                                                provider.clone(),
                                                owner,
                                                new_block.block_number,
                                                None,
                                            ) {
                                                Ok(profit) => info!(
                                                    "Simulation was successful. Profit: {:?}",
                                                    profit
                                                ),
                                                Err(e) => {
                                                    info!("Simulation failed. Error: {:?}", e)
                                                }
                                            }
                                        }
                                        None => {}
                                    }
                                }
                            }
                        }
                        Err(_) => {}
                    }
                }
                Event::Log(_) => {}
            },
            Err(_) => {}
        }
    }
}
