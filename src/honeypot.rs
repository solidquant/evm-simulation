use ethers::types::{Block, BlockId, BlockNumber, H160, H256, U256, U64};
use ethers_providers::Middleware;
use log::info;
use std::{collections::HashMap, path::Path, str::FromStr, sync::Arc};

use crate::pools::Pool;
use crate::simulator::EvmSimulator;
use crate::tokens::{get_implementation, get_token_info, Token};
use crate::trace::EvmTracer;

#[derive(Debug, Clone)]
pub struct SafeTokens {
    pub weth: H160,
    pub usdt: H160,
    pub usdc: H160,
    pub dai: H160,
}

impl SafeTokens {
    pub fn new() -> Self {
        Self {
            usdt: H160::from_str("0xdAC17F958D2ee523a2206206994597C13D831ec7").unwrap(),
            weth: H160::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap(),
            usdc: H160::from_str("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap(),
            dai: H160::from_str("0x6B175474E89094C44Da98b954EedeAC495271d0F").unwrap(),
        }
    }
}

pub struct HoneypotFilter<M> {
    pub simulator: EvmSimulator<M>,
    pub safe_tokens: SafeTokens,
    pub token_info: HashMap<H160, Token>,
    pub safe_token_info: HashMap<H160, Token>,
    pub balance_slots: HashMap<H160, u32>,
    pub honeypot: HashMap<H160, bool>,
}

impl<M: Middleware + 'static> HoneypotFilter<M> {
    pub fn new(provider: Arc<M>, block: Block<H256>) -> Self {
        let owner = H160::from_str("0x001a06BF8cE4afdb3f5618f6bafe35e9Fc09F187").unwrap();
        let simulator = EvmSimulator::new(provider.clone(), owner, block.number.unwrap());
        let safe_tokens = SafeTokens::new();
        let token_info = HashMap::new();
        let safe_token_info = HashMap::new();
        let balance_slots = HashMap::new();
        let honeypot = HashMap::new();
        Self {
            simulator,
            safe_tokens,
            token_info,
            safe_token_info,
            balance_slots,
            honeypot,
        }
    }

    pub async fn setup(&mut self) {
        // Get safe_token_info using the four following tokens that are widely used as safe tokens
        let provider = &self.simulator.provider;
        let owner = self.simulator.owner;
        let block_number = &self.simulator.block_number;

        let tracer = EvmTracer::new(provider.clone());

        let chain_id = provider.get_chainid().await.unwrap();
        let nonce = self
            .simulator
            .provider
            .get_transaction_count(
                owner,
                Some(BlockId::Number(BlockNumber::Number(*block_number))),
            )
            .await
            .unwrap();

        for token in vec![
            self.safe_tokens.usdt,
            self.safe_tokens.weth,
            self.safe_tokens.usdc,
            self.safe_tokens.dai,
        ] {
            if !self.safe_token_info.contains_key(&token) {
                match tracer
                    .find_balance_slot(
                        token,
                        owner,
                        nonce,
                        U64::from(chain_id.as_u64()),
                        block_number.as_u64(),
                    )
                    .await
                {
                    Ok(slot) => {
                        if slot.0 {
                            self.balance_slots.insert(token, slot.1);
                            let mut info = get_token_info(provider.clone(), token).await.unwrap();
                            info!("{} ({:?}): {:?}", info.name, token, slot.1);
                            match get_implementation(provider.clone(), token, *block_number).await {
                                Ok(implementation) => info.add_implementation(implementation),
                                Err(_) => {}
                            }
                            self.safe_token_info.insert(token, info);
                        }
                    }
                    Err(_) => {}
                }
            }
        }
    }

    pub async fn filter_tokens(&mut self, pools: &Vec<Pool>) {
        // load cached
        let token_file_path = Path::new("src/.cached-tokens.csv");
        let honeypot_file_path = Path::new("src/.cached-honeypot.csv");

        if token_file_path.exists() {
            let mut reader = csv::Reader::from_path(token_file_path).unwrap();
            for row in reader.records() {
                let row = row.unwrap();
                let token = Token::from(row);
                self.token_info.insert(token.address, token);
            }
        }
        info!("✔️ Loaded {:?} token info from cache", self.token_info.len());

        if honeypot_file_path.exists() {
            let mut reader = csv::Reader::from_path(honeypot_file_path).unwrap();
            for row in reader.records() {
                let row = row.unwrap();
                let honeypot_address = H160::from_str(row.get(0).unwrap()).unwrap();
                self.honeypot.insert(honeypot_address, true);
            }
        }
        info!(
            "✔️ Loaded {:?} honeypot info from cache",
            self.honeypot.len()
        );

        self.simulator.deploy_simulator();

        for (idx, pool) in pools.iter().enumerate() {
            let token0_is_safe = self.safe_token_info.contains_key(&pool.token0);
            let token1_is_safe = self.safe_token_info.contains_key(&pool.token1);

            if token0_is_safe && token1_is_safe {
                continue;
            }

            // only test for token if it's a match with either of the safe tokens
            if token0_is_safe || token1_is_safe {
                let (safe_token, test_token) = if token0_is_safe {
                    (pool.token0, pool.token1)
                } else {
                    (pool.token1, pool.token0)
                };

                if self.token_info.contains_key(&test_token)
                    || self.honeypot.contains_key(&test_token)
                {
                    // skip if test_tokens was already tested
                    continue;
                }

                // We take extra measures to filter out the pools with too little liquidity
                // Using the below amount to test swaps, we know that there's enough liquidity in the pool
                let mut amount_in_u32 = 1;

                if safe_token == self.safe_tokens.weth {
                    amount_in_u32 = 20;
                } else if safe_token == self.safe_tokens.usdt {
                    amount_in_u32 = 10000;
                } else if safe_token == self.safe_tokens.usdc {
                    amount_in_u32 = 10000;
                } else if safe_token == self.safe_tokens.dai {
                    amount_in_u32 = 10000
                }

                // seed the simulator with some safe token balance
                let safe_token_info = self.safe_token_info.get(&safe_token).unwrap();
                let safe_token_slot = self.balance_slots.get(&safe_token).unwrap();

                self.simulator.set_token_balance(
                    self.simulator.simulator_address,
                    safe_token,
                    safe_token_info.decimals,
                    *safe_token_slot,
                    amount_in_u32,
                );

                info!(
                    "✅ [{}] {} -> {:?}",
                    idx, safe_token_info.symbol, test_token
                );

                let amount_in = U256::from(amount_in_u32)
                    .checked_mul(U256::from(10).pow(U256::from(safe_token_info.decimals)))
                    .unwrap();

                // Buy Test
                let buy_output = self.simulator.v2_simulate_swap(
                    amount_in,
                    pool.address,
                    safe_token,
                    test_token,
                    true,
                );
                let out = match buy_output {
                    Ok(out) => out,
                    Err(e) => {
                        info!("<BUY ERROR> {:?}", e);
                        self.honeypot.insert(test_token, true);
                        continue;
                    }
                };

                if out.0 == out.1 {
                    // Sell Test
                    let amount_in = out.1;
                    let sell_output = self.simulator.v2_simulate_swap(
                        amount_in,
                        pool.address,
                        test_token,
                        safe_token,
                        true,
                    );
                    let out = match sell_output {
                        Ok(out) => out,
                        Err(e) => {
                            info!("<SELL ERROR> {:?}", e);
                            self.honeypot.insert(test_token, true);
                            continue;
                        }
                    };

                    if out.0 == out.1 {
                        match get_token_info(self.simulator.provider.clone(), test_token).await {
                            Ok(info) => {
                                info!(
                                    "Added safe token info ({}). Total: {:?} tokens",
                                    info.symbol,
                                    self.token_info.len()
                                );
                                self.token_info.insert(test_token, info);
                            }
                            Err(_) => {}
                        }
                    } else {
                        self.honeypot.insert(test_token, true);
                    }
                } else {
                    self.honeypot.insert(test_token, true);
                }
            }
        }

        // cache to csv files
        let mut token_writer = csv::Writer::from_path(token_file_path).unwrap();
        for (_, info) in &self.token_info {
            token_writer.serialize(info.cache_row()).unwrap();
        }
        token_writer.flush().unwrap();

        let mut honeypot_writer = csv::Writer::from_path(honeypot_file_path).unwrap();
        for (token, _) in &self.honeypot {
            honeypot_writer.serialize(token).unwrap();
        }
        honeypot_writer.flush().unwrap();
    }
}
