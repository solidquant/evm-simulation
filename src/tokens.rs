use anyhow::Result;
use csv::StringRecord;
use ethers::{abi::parse_abi, prelude::*};
use ethers_contract::{Contract, Multicall};
use ethers_core::types::{BlockId, BlockNumber, TxHash, H160, U256};
use std::{str::FromStr, sync::Arc};
use tokio::task::JoinSet;

use crate::constants::{IMPLEMENTATION_SLOTS, ZERO_ADDRESS};

#[derive(Debug, Clone)]
pub struct Token {
    pub address: H160,
    pub implementation: Option<H160>,
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
}

impl From<StringRecord> for Token {
    fn from(record: StringRecord) -> Self {
        Self {
            address: H160::from_str(record.get(0).unwrap()).unwrap(),
            implementation: match record.get(1) {
                Some(raw_impl_addr) => match raw_impl_addr {
                    "" => None,
                    _ => Some(H160::from_str(raw_impl_addr).unwrap()),
                },
                None => None,
            },
            name: String::from(record.get(2).unwrap()),
            symbol: String::from(record.get(3).unwrap()),
            decimals: record.get(4).unwrap().parse::<u8>().unwrap(),
        }
    }
}

impl Token {
    pub fn add_implementation(&mut self, implementation: Option<H160>) {
        self.implementation = implementation;
    }

    pub fn cache_row(&self) -> (String, String, String, String, u8) {
        (
            format!("{:?}", self.address),
            match self.implementation {
                Some(implementation) => format!("{:?}", implementation),
                None => String::from(""),
            },
            self.name.clone(),
            self.symbol.clone(),
            self.decimals,
        )
    }
}

pub async fn get_implementation<M: Middleware + 'static>(
    provider: Arc<M>,
    token: H160,
    block_number: U64,
) -> Result<Option<H160>> {
    let mut set = JoinSet::new();

    for slot in IMPLEMENTATION_SLOTS.iter() {
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
