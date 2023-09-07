use anyhow::Result;
use bytes::Bytes as OutputBytes;
use ethers::abi::parse_abi;
use ethers::prelude::BaseContract;
use ethers::types::{Bytes, H160, U256};

#[derive(Clone)]
pub struct TokenABI {
    pub abi: BaseContract,
}

impl TokenABI {
    pub fn new() -> Self {
        let abi = BaseContract::from(
            parse_abi(&[
                "function balanceOf(address) external view returns (uint256)",
                "function approve(address spender, uint256 value) external view returns (bool)",
            ])
            .unwrap(),
        );
        Self { abi }
    }

    pub fn balance_of_input(&self, account: H160) -> Result<Bytes> {
        let calldata = self.abi.encode("balanceOf", account)?;
        Ok(calldata)
    }

    pub fn balance_of_output(&self, output: OutputBytes) -> Result<U256> {
        let out = self.abi.decode_output("balanceOf", output)?;
        Ok(out)
    }

    pub fn approve_input(&self, spender: H160) -> Result<Bytes> {
        let calldata = self.abi.encode("approve", (spender, U256::MAX))?;
        Ok(calldata)
    }

    pub fn approve_output(&self, output: OutputBytes) -> Result<bool> {
        let out = self.abi.decode_output("approve", output)?;
        Ok(out)
    }
}
