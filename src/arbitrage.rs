use anyhow::Result;
use ethers::types::{H160, U256, U64};
use ethers_providers::Middleware;
use foundry_evm::{executor::fork::SharedBackend, revm::db::CacheDB};
use log::info;
use std::sync::Arc;

use crate::paths::ArbPath;
use crate::simulator::EvmSimulator;
use crate::tokens::Token;

#[derive(Debug, Clone)]
pub struct TriangularArbitrage {
    pub amount_in: U256,
    pub path: ArbPath,
    pub balance_slot: u32,
    pub target_token: Token,
}

pub fn simulate_triangular_arbitrage<M: Middleware + 'static>(
    arb: TriangularArbitrage,
    provider: Arc<M>,
    owner: H160,
    block_number: U64,
    fork_db: Option<CacheDB<SharedBackend>>,
) -> Result<i128> {
    info!("\n[🔮 Arbitrage Path Simulation]");

    let target_token = arb.target_token;

    let mut simulator = EvmSimulator::new(provider, owner, block_number);
    let simulator_address = simulator.simulator_address;
    match fork_db {
        Some(db) => simulator.inject_db(db),
        None => {
            simulator.set_eth_balance(100000);
            simulator.deploy_simulator();
            simulator.set_token_balance(
                simulator_address,
                target_token.address,
                target_token.decimals,
                arb.balance_slot,
                100000,
            );
        }
    }

    let mut amount_out = arb.amount_in;

    for n in 0..arb.path.nhop {
        let pool = arb.path.get_pool(n);
        let zero_for_one = arb.path.get_zero_for_one(n);
        let (input_token, output_token) = if zero_for_one {
            (pool.token0, pool.token1)
        } else {
            (pool.token1, pool.token0)
        };

        let out = simulator.v2_simulate_swap(
            amount_out,
            pool.address,
            input_token,
            output_token,
            true,
        )?;
        amount_out = out.1;
        info!("✅ Swap #{}: {:?}", n + 1, amount_out);
    }

    let profit = (amount_out.as_u64() as i128) - (arb.amount_in.as_u64() as i128);
    let divisor = (10.0 as f64).powi(target_token.decimals as i32);
    let profit_in_target_token = (profit as f64) / divisor;
    info!(
        "▶️ Profit: {:?} {}",
        profit_in_target_token, target_token.symbol
    );

    Ok(profit)
}
