use anyhow::{anyhow, Result};
use ethers::{
    abi::{self, parse_abi},
    prelude::*,
    types::transaction::eip2930::AccessList,
};
use ethers_providers::Middleware;
use foundry_evm::revm::primitives::keccak256;
use std::sync::Arc;

pub struct EvmTracer<M> {
    provider: Arc<M>,
}

impl<M: Middleware + 'static> EvmTracer<M> {
    pub fn new(provider: Arc<M>) -> Self {
        Self { provider }
    }

    pub async fn get_state_diff(
        &self,
        tx: Eip1559TransactionRequest,
        block_number: u64,
    ) -> Result<GethTrace> {
        let trace = self
            .provider
            .debug_trace_call(
                tx,
                Some(BlockId::Number(BlockNumber::Number(block_number.into()))),
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
            .await
            .unwrap();

        Ok(trace)
    }

    pub async fn find_balance_slot(
        &self,
        token: H160,
        owner: H160,
        nonce: U256,
        chain_id: U64,
        block_number: u64,
    ) -> Result<(bool, u32)> {
        // A brute force way of finding the storage slot value of an ERC-20 token
        // Calling balanceOf and tracing the call using "debug_traceCall" will give us access to the
        // storage slot of "balances"
        let erc20_contract = BaseContract::from(
            parse_abi(&["function balanceOf(address) external view returns (uint256)"]).unwrap(),
        );
        let calldata = erc20_contract.encode("balanceOf", owner).unwrap();
        let tx = Eip1559TransactionRequest {
            to: Some(NameOrAddress::Address(token)),
            from: Some(owner),
            data: Some(calldata.0.into()),
            value: Some(U256::zero()),
            chain_id: Some(chain_id),
            max_priority_fee_per_gas: None,
            max_fee_per_gas: None,
            gas: None,
            nonce: Some(nonce),
            access_list: AccessList::default(),
        };
        let trace = self.get_state_diff(tx, block_number).await.unwrap();
        match trace {
            GethTrace::Known(known) => match known {
                GethTraceFrame::PreStateTracer(prestate) => match prestate {
                    PreStateFrame::Default(prestate_mode) => {
                        let token_info =
                            prestate_mode.0.get(&token).ok_or(anyhow!("no token key"))?;
                        let touched_storage = token_info
                            .storage
                            .clone()
                            .ok_or(anyhow!("no storage values"))?;
                        for i in 0..20 {
                            let slot = keccak256(&abi::encode(&[
                                abi::Token::Address(owner.into()),
                                abi::Token::Uint(U256::from(i)),
                            ]));
                            match touched_storage.get(&slot.into()) {
                                Some(_) => {
                                    return Ok((true, i));
                                }
                                None => {}
                            }
                        }
                        Ok((false, 0))
                    }
                    _ => Ok((false, 0)),
                },
                _ => Ok((false, 0)),
            },
            _ => Ok((false, 0)),
        }
    }

    pub async fn find_v2_reserves_slot(
        &self,
        pool: H160,
        owner: H160,
        nonce: U256,
        chain_id: U64,
        block_number: u64,
    ) -> Result<(bool, u32)> {
        let v2_pool_contract = BaseContract::from(
            parse_abi(&["function getReserves() external view returns (uint112,uint112,uint32)"])
                .unwrap(),
        );
        let calldata = v2_pool_contract.encode("getReserves", ()).unwrap();
        let tx = Eip1559TransactionRequest {
            to: Some(NameOrAddress::Address(pool)),
            from: Some(owner),
            data: Some(calldata.0.into()),
            value: Some(U256::zero()),
            chain_id: Some(chain_id),
            max_priority_fee_per_gas: None,
            max_fee_per_gas: None,
            gas: None,
            nonce: Some(nonce),
            access_list: AccessList::default(),
        };
        let trace = self.get_state_diff(tx, block_number).await.unwrap();
        match trace {
            GethTrace::Known(known) => match known {
                GethTraceFrame::PreStateTracer(prestate) => match prestate {
                    PreStateFrame::Default(prestate_mode) => {
                        let token_info =
                            prestate_mode.0.get(&pool).ok_or(anyhow!("no token key"))?;
                        let touched_storage = token_info
                            .storage
                            .clone()
                            .ok_or(anyhow!("no storage values"))?;
                        let slot = touched_storage
                            .keys()
                            .next()
                            .ok_or(anyhow!("no slot value in storage"))?;
                        Ok((true, slot.to_low_u64_be() as u32))
                    }
                    _ => Ok((false, 0)),
                },
                _ => Ok((false, 0)),
            },
            _ => Ok((false, 0)),
        }
    }
}
