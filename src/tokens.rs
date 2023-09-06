use anyhow::Result;
use ethers::{abi::parse_abi, prelude::*};
use ethers_contract::{Contract, Multicall};
use ethers_core::types::{BlockId, BlockNumber, TxHash, H160, U256};
use ethers_providers::{Provider, Ws};
use std::sync::Arc;
use tokio::task::JoinSet;

use crate::constants::ZERO_ADDRESS;

#[derive(Debug, Clone)]
pub struct Token {
    pub address: H160,
    pub implementation: Option<H160>,
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
}

impl Token {
    pub fn add_implementation(&mut self, implementation: Option<H160>) {
        self.implementation = implementation;
    }
}

pub async fn get_implementation<M: Middleware + 'static>(
    provider: Arc<M>,
    token: H160,
    block_number: U64,
) -> Result<Option<H160>> {
    // adapted from: https://github.com/gnosis/evm-proxy-detection/blob/main/src/index.ts
    let eip_1967_logic_slot =
        U256::from("0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc");
    let eip_1967_beacon_slot =
        U256::from("0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50");
    let open_zeppelin_implementation_slot =
        U256::from("0x7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c3");
    let eip_1822_logic_slot =
        U256::from("0xc5f16f0fcc639fa48a6947836d9850f504798523bf8c9a3a87d5876cf622bcf7");

    let implementation_slots = vec![
        eip_1967_logic_slot,
        eip_1967_beacon_slot,
        open_zeppelin_implementation_slot,
        eip_1822_logic_slot,
    ];

    let mut set = JoinSet::new();

    for slot in implementation_slots {
        let _provider = provider.clone();
        let fut = tokio::spawn(async move {
            _provider
                .get_storage_at(
                    token,
                    TxHash::from_uint(&slot),
                    Some(BlockId::Number(BlockNumber::Number(block_number))),
                )
                .await
        });
        set.spawn(fut);
    }

    while let Some(res) = set.join_next().await {
        let out = res???;
        let implementation = H160::from(out);
        if implementation != *ZERO_ADDRESS {
            return Ok(Some(implementation));
        }
    }

    Ok(None)
}

pub async fn get_token_info<M: Middleware + 'static>(
    provider: Arc<M>,
    token: H160,
) -> Result<Token> {
    let erc20_contract = BaseContract::from(
        parse_abi(&[
            "function name() external view returns (string)",
            "function symbol() external view returns (string)",
            "function decimals() external view returns (uint8)",
        ])
        .unwrap(),
    );

    let mut multicall = Multicall::new(provider.clone(), None).await?;
    let contract = Contract::new(token, erc20_contract.abi().clone(), provider.clone());

    let name_call = contract.method::<_, String>("name", ())?;
    let symbol_call = contract.method::<_, String>("symbol", ())?;
    let decimals_call = contract.method::<_, u8>("decimals", ())?;

    multicall.add_call(name_call, true);
    multicall.add_call(symbol_call, true);
    multicall.add_call(decimals_call, true);

    let result: (String, String, u8) = multicall.call().await?;
    let token_info = Token {
        address: token,
        implementation: None,
        name: result.0,
        symbol: result.1,
        decimals: result.2,
    };

    Ok(token_info)
}
