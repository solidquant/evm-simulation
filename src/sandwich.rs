use anyhow::Result;
use ethers::types::{Transaction, H160, U256, U64};
use ethers_providers::Middleware;
use foundry_evm::{executor::fork::SharedBackend, revm::db::CacheDB};
use log::info;
use std::{collections::HashMap, sync::Arc};

use crate::honeypot::HoneypotFilter;
use crate::pools::Pool;
use crate::simulator::EvmSimulator;
use crate::tokens::Token;

#[derive(Debug, Clone)]
pub struct Sandwich {
    pub amount_in: U256,
    pub balance_slot: u32,
    pub target_token: Token,
    pub target_pool: Pool,
    pub meat_tx: Transaction,
}

pub struct SandwichSimulator<M> {
    pub simulator: EvmSimulator<M>,
}

impl<M: Middleware + 'static> SandwichSimulator<M> {
    pub fn new(provider: Arc<M>, owner: H160, block_number: U64) -> Self {
        let simulator = EvmSimulator::new(provider, owner, block_number);
        Self { simulator }
    }

    pub fn db_snapshot(&mut self) -> CacheDB<SharedBackend> {
        self.simulator.evm.db.as_mut().unwrap().clone()
    }

    pub async fn simulate(
        &mut self,
        tx: &Transaction,
        sandwichable_pools: &HashMap<H160, Option<H160>>,
        verified_pools_map: &HashMap<H160, Pool>,
        honeypot_filter: &HoneypotFilter<M>,
    ) -> Result<()> {
        // Setup DB and retrieve storage values required to run simulation
        self.simulator.set_eth_balance(10000);
        self.simulator.deploy_simulator();

        let mut sandwiches = Vec::new();

        for (touched_pool, used_token) in sandwichable_pools {
            // if used_token is not None, we can sandwich this tx
            match used_token {
                Some(safe_token) => {
                    // seed simulator contract with some used_token balance
                    let simulator_address = self.simulator.simulator_address;
                    let token_info = honeypot_filter.safe_token_info.get(safe_token).unwrap();
                    let balance_slot = honeypot_filter.balance_slots.get(safe_token).unwrap();
                    self.simulator.set_token_balance(
                        simulator_address,
                        *safe_token,
                        token_info.decimals,
                        *balance_slot,
                        10000,
                    );

                    // load storage values before cloning db
                    // storage values required to simulate swap: token0/token1 balance & pool reserves
                    let pool = verified_pools_map.get(touched_pool).unwrap();
                    _ = self
                        .simulator
                        .token_balance_of(pool.token0, simulator_address);
                    _ = self
                        .simulator
                        .token_balance_of(pool.token1, simulator_address);
                    _ = self.simulator.v2_pool_get_reserves(*touched_pool);

                    let sandwich = Sandwich {
                        amount_in: U256::zero(),
                        balance_slot: *balance_slot,
                        target_token: token_info.clone(),
                        target_pool: pool.clone(),
                        meat_tx: tx.clone(),
                    };
                    sandwiches.push(sandwich);
                }
                None => {}
            }
        }

        // Clone the DB and inject it into simulator to run multiple bundles in parallel
        let fork_db = self.db_snapshot();

        // Try running simulations one by one at first
        for mut sandwich in sandwiches {
            let amount_in =
                U256::from(1) * U256::from(10).pow(U256::from(sandwich.target_token.decimals));
            sandwich.amount_in = amount_in;
            match simulate_sandwich_bundle(
                sandwich.clone(),
                self.simulator.provider.clone(),
                self.simulator.owner,
                self.simulator.block_number,
                Some(fork_db.clone()),
            ) {
                Ok(_) => {}
                Err(e) => info!("[SIMULATION ERROR] {:?} {:?}", sandwich, e),
            };
        }

        Ok(())
    }
}

pub fn simulate_sandwich_bundle<M: Middleware + 'static>(
    sandwich: Sandwich,
    provider: Arc<M>,
    owner: H160,
    block_number: U64,
    fork_db: Option<CacheDB<SharedBackend>>,
) -> Result<i128> {
    // Create a simulator instance and inject the forked db
    let amount_in = sandwich.amount_in;
    let target_token = sandwich.target_token;
    let target_pool = sandwich.target_pool;

    info!("\n[🔮 Sandwich Bundle Simulation]");
    info!(
        "- Pool: {:?} / Token: {:?}",
        target_pool.address, target_token.symbol
    );
    info!("- Amount in: {:?} {:?}", amount_in, target_token.symbol);

    let (input_token, output_token) = if target_pool.token0 == target_token.address {
        (target_pool.token0, target_pool.token1)
    } else {
        (target_pool.token1, target_pool.token0)
    };

    let mut simulator = EvmSimulator::new(provider, owner, block_number);
    let simulator_address = simulator.simulator_address;
    match fork_db {
        Some(db) => simulator.inject_db(db),
        None => {
            simulator.set_eth_balance(10000);
            simulator.deploy_simulator();
            simulator.set_token_balance(
                simulator_address,
                target_token.address,
                target_token.decimals,
                sandwich.balance_slot,
                10000,
            );
        }
    }

    // Frontrun tx
    let frontrun_out = simulator.v2_simulate_swap(
        amount_in,
        target_pool.address,
        input_token,
        output_token,
        true,
    )?;
    info!("✅ Frontrun out: {:?}", frontrun_out.1);

    // Meat tx
    match simulator.run_pending_tx(&sandwich.meat_tx) {
        Ok(_) => info!("✅ Meat TX Successful"),
        Err(e) => info!("✖️ Meat TX Failed: {:?}", e),
    }

    // Backrun tx
    let backrun_out = simulator.v2_simulate_swap(
        frontrun_out.1,
        target_pool.address,
        output_token,
        input_token,
        true,
    )?;
    info!("✅ Backrun out: {:?}", backrun_out.1);

    let amount_out = backrun_out.1;
    let profit = (amount_out.as_u64() as i128) - (amount_in.as_u64() as i128);
    info!("▶️ Profit: {:?} {:?}", profit, target_token.symbol);

    Ok(profit)
}
