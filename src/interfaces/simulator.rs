use anyhow::Result;
use bytes::Bytes as OutputBytes;
use ethers::abi::parse_abi;
use ethers::prelude::BaseContract;
use ethers::types::{Bytes, H160, U256};

#[derive(Clone)]
pub struct SimulatorABI {
    pub abi: BaseContract,
}

impl SimulatorABI {
    pub fn new() -> Self {
        let abi = BaseContract::from(
            parse_abi(&[
                "function v2SimulateSwap(uint256,address,address,address) external returns (uint256, uint256)",
                "function getAmountOut(uint256,uint256,uint256) external returns (uint256)",
            ]).unwrap()
        );
        Self { abi }
    }

    pub fn v2_simulate_swap_input(
        &self,
        amount_in: U256,
        target_pool: H160,
        input_token: H160,
        output_token: H160,
    ) -> Result<Bytes> {
        let calldata = self.abi.encode(
            "v2SimulateSwap",
            (amount_in, target_pool, input_token, output_token),
        )?;
        Ok(calldata)
    }

    pub fn v2_simulate_swap_output(&self, output: OutputBytes) -> Result<(U256, U256)> {
        let out = self.abi.decode_output("v2SimulateSwap", output)?;
        Ok(out)
    }

    pub fn get_amount_out_input(
        &self,
        amount_in: U256,
        reserve_in: U256,
        reserve_out: U256,
    ) -> Result<Bytes> {
        let calldata = self
            .abi
            .encode("getAmountOut", (amount_in, reserve_in, reserve_out))?;
        Ok(calldata)
    }

    pub fn get_amount_out_output(&self, output: OutputBytes) -> Result<U256> {
        let out = self.abi.decode_output("getAmountOut", output)?;
        Ok(out)
    }
}
