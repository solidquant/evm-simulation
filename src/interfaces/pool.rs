use anyhow::Result;
use bytes::Bytes as OutputBytes;
use ethers::abi::parse_abi;
use ethers::prelude::BaseContract;
use ethers::types::Bytes;

#[derive(Clone)]
pub struct V2PoolABI {
    pub abi: BaseContract,
}

impl V2PoolABI {
    pub fn new() -> Self {
        let abi = BaseContract::from(
            parse_abi(&["function getReserves() external view returns (uint112,uint112,uint32)"])
                .unwrap(),
        );
        Self { abi }
    }

    pub fn get_reserves_input(&self) -> Result<Bytes> {
        let calldata = self.abi.encode("getReserves", ())?;
        Ok(calldata)
    }

    pub fn get_reserves_output(&self, output: OutputBytes) -> Result<(u128, u128, u32)> {
        let out = self.abi.decode_output("getReserves", output)?;
        Ok(out)
    }
}
