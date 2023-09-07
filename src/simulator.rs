use anyhow::{anyhow, Result};
use bytes::Bytes;
use ethers::abi;
use ethers::types::{Transaction, H160, U256, U64};
use ethers_providers::Middleware;
use foundry_evm::{
    executor::{
        fork::{BlockchainDb, BlockchainDbMeta, SharedBackend},
        Bytecode, ExecutionResult, Output, TransactTo,
    },
    revm::{
        db::{CacheDB, Database},
        primitives::{keccak256, AccountInfo, U256 as rU256},
        EVM,
    },
};
use std::{collections::BTreeSet, str::FromStr, sync::Arc};

use crate::constants::SIMULATOR_CODE;
use crate::interfaces::{pool::V2PoolABI, simulator::SimulatorABI, token::TokenABI};

#[derive(Clone)]
pub struct EvmSimulator<M> {
    pub provider: Arc<M>,
    pub owner: H160,
    pub evm: EVM<CacheDB<SharedBackend>>,
    pub block_number: U64,

    pub token: TokenABI,
    pub v2_pool: V2PoolABI,
    pub simulator: SimulatorABI,

    pub simulator_address: H160,
}

#[derive(Debug, Clone)]
pub struct Tx {
    pub caller: H160,
    pub transact_to: H160,
    pub data: Bytes,
    pub value: U256,
    pub gas_limit: u64,
}

#[derive(Debug, Clone)]
pub struct TxResult {
    pub output: Bytes,
    pub gas_used: u64,
    pub gas_refunded: u64,
}

impl<M: Middleware + 'static> EvmSimulator<M> {
    pub fn new(provider: Arc<M>, owner: H160, block_number: U64) -> Self {
        let shared_backend = SharedBackend::spawn_backend_thread(
            provider.clone(),
            BlockchainDb::new(
                BlockchainDbMeta {
                    cfg_env: Default::default(),
                    block_env: Default::default(),
                    hosts: BTreeSet::from(["".to_string()]),
                },
                None,
            ),
            Some(block_number.into()),
        );
        let db = CacheDB::new(shared_backend);

        let mut evm = EVM::new();
        evm.database(db);

        evm.env.cfg.limit_contract_code_size = Some(0x100000);
        evm.env.cfg.disable_block_gas_limit = true;
        evm.env.cfg.disable_base_fee = true;

        evm.env.block.number = rU256::from(block_number.as_u64() + 1);

        Self {
            provider,
            owner,
            evm,
            block_number,

            token: TokenABI::new(),
            v2_pool: V2PoolABI::new(),
            simulator: SimulatorABI::new(),

            simulator_address: H160::from_str("0x4E17607Fb72C01C280d7b5c41Ba9A2109D74a32C")
                .unwrap(),
        }
    }

    pub fn inject_db(&mut self, db: CacheDB<SharedBackend>) {
        self.evm.database(db);
    }

    pub fn run_pending_tx(&mut self, tx: &Transaction) -> Result<TxResult> {
        // We simply need to commit changes to the DB
        self.evm.env.tx.caller = tx.from.0.into();
        self.evm.env.tx.transact_to = TransactTo::Call(tx.to.unwrap_or_default().0.into());
        self.evm.env.tx.data = tx.input.0.clone();
        self.evm.env.tx.value = tx.value.into();
        self.evm.env.tx.chain_id = tx.chain_id.map(|id| id.as_u64());
        self.evm.env.tx.gas_limit = tx.gas.as_u64();

        match tx.transaction_type {
            Some(U64([0])) => self.evm.env.tx.gas_price = tx.gas_price.unwrap_or_default().into(),
            Some(_) => {
                self.evm.env.tx.gas_priority_fee =
                    tx.max_priority_fee_per_gas.map(|mpf| mpf.into());
                self.evm.env.tx.gas_price = tx.max_fee_per_gas.unwrap_or_default().into();
            }
            None => self.evm.env.tx.gas_price = tx.gas_price.unwrap_or_default().into(),
        }

        let result = match self.evm.transact_commit() {
            Ok(result) => result,
            Err(e) => return Err(anyhow!("EVM call failed: {:?}", e)),
        };

        let output = match result {
            ExecutionResult::Success {
                gas_used,
                gas_refunded,
                output,
                ..
            } => match output {
                Output::Call(o) => TxResult {
                    output: o,
                    gas_used,
                    gas_refunded,
                },
                Output::Create(o, _) => TxResult {
                    output: o,
                    gas_used,
                    gas_refunded,
                },
            },
            ExecutionResult::Revert { gas_used, output } => {
                return Err(anyhow!(
                    "EVM REVERT: {:?} / Gas used: {:?}",
                    output,
                    gas_used
                ))
            }
            ExecutionResult::Halt { reason, .. } => return Err(anyhow!("EVM HALT: {:?}", reason)),
        };

        Ok(output)
    }

    pub fn _call(&mut self, tx: Tx, commit: bool) -> Result<TxResult> {
        self.evm.env.tx.caller = tx.caller.into();
        self.evm.env.tx.transact_to = TransactTo::Call(tx.transact_to.into());
        self.evm.env.tx.data = tx.data;
        self.evm.env.tx.value = tx.value.into();
        self.evm.env.tx.gas_limit = 5000000;

        let result;

        if commit {
            result = match self.evm.transact_commit() {
                Ok(result) => result,
                Err(e) => return Err(anyhow!("EVM call failed: {:?}", e)),
            };
        } else {
            let ref_tx = self
                .evm
                .transact_ref()
                .map_err(|e| anyhow!("EVM staticcall failed: {:?}", e))?;
            result = ref_tx.result;
        }

        let output = match result {
            ExecutionResult::Success {
                gas_used,
                gas_refunded,
                output,
                ..
            } => match output {
                Output::Call(o) => TxResult {
                    output: o,
                    gas_used,
                    gas_refunded,
                },
                Output::Create(o, _) => TxResult {
                    output: o,
                    gas_used,
                    gas_refunded,
                },
            },
            ExecutionResult::Revert { gas_used, output } => {
                return Err(anyhow!(
                    "EVM REVERT: {:?} / Gas used: {:?}",
                    output,
                    gas_used
                ))
            }
            ExecutionResult::Halt { reason, .. } => return Err(anyhow!("EVM HALT: {:?}", reason)),
        };

        Ok(output)
    }

    pub fn staticcall(&mut self, tx: Tx) -> Result<TxResult> {
        self._call(tx, false)
    }

    pub fn call(&mut self, tx: Tx) -> Result<TxResult> {
        self._call(tx, true)
    }

    pub fn get_eth_balance(&mut self) -> U256 {
        let acc = self
            .evm
            .db
            .as_mut()
            .unwrap()
            .basic(self.owner.into())
            .unwrap()
            .unwrap();
        acc.balance.into()
    }

    pub fn set_eth_balance(&mut self, balance: u32) {
        let user_balance = rU256::from(balance)
            .checked_mul(rU256::from(10).pow(rU256::from(18)))
            .unwrap();
        let user_info = AccountInfo::new(user_balance, 0, Bytecode::default());
        self.evm
            .db
            .as_mut()
            .unwrap()
            .insert_account_info(self.owner.into(), user_info);
    }

    // ERC-20 Token functions
    pub fn set_token_balance(
        &mut self,
        account: H160,
        token: H160,
        decimals: u8,
        slot: u32,
        balance: u32,
    ) {
        let slot = keccak256(&abi::encode(&[
            abi::Token::Address(account.into()),
            abi::Token::Uint(U256::from(slot)),
        ]));
        let target_balance = rU256::from(balance)
            .checked_mul(rU256::from(10).pow(rU256::from(decimals)))
            .unwrap();
        self.evm
            .db
            .as_mut()
            .unwrap()
            .insert_account_storage(token.into(), slot.into(), target_balance)
            .unwrap();
    }

    pub fn token_balance_of(&mut self, token: H160, account: H160) -> Result<U256> {
        let calldata = self.token.balance_of_input(account)?;
        let value = self.staticcall(Tx {
            caller: self.owner.into(),
            transact_to: token,
            data: calldata.0,
            value: U256::zero(),
            gas_limit: 0,
        })?;
        let out = self.token.balance_of_output(value.output)?;
        Ok(out)
    }

    // V2 Pool functions
    pub fn set_v2_pool_reserves(&mut self, pool: H160, reserves: rU256) {
        let slot = rU256::from(8);
        self.evm
            .db
            .as_mut()
            .unwrap()
            .insert_account_storage(pool.into(), slot.into(), reserves)
            .unwrap();
    }

    pub fn v2_pool_get_reserves(&mut self, pool: H160) -> Result<(u128, u128, u32)> {
        let calldata = self.v2_pool.get_reserves_input()?;
        let value = self.staticcall(Tx {
            caller: self.owner,
            transact_to: pool,
            data: calldata.0,
            value: U256::zero(),
            gas_limit: 0,
        })?;
        let out = self.v2_pool.get_reserves_output(value.output)?;
        Ok(out)
    }

    // Simulator functions
    pub fn deploy_simulator(&mut self) {
        let contract_info = AccountInfo::new(
            rU256::ZERO,
            0,
            Bytecode::new_raw((*SIMULATOR_CODE.0).into()),
        );
        self.evm
            .db
            .as_mut()
            .unwrap()
            .insert_account_info(self.simulator_address.into(), contract_info);
    }

    pub fn v2_simulate_swap(
        &mut self,
        amount_in: U256,
        target_pool: H160,
        input_token: H160,
        output_token: H160,
        commit: bool,
    ) -> Result<(U256, U256)> {
        let calldata = self.simulator.v2_simulate_swap_input(
            amount_in,
            target_pool,
            input_token,
            output_token,
        )?;
        let tx = Tx {
            caller: self.owner,
            transact_to: self.simulator_address,
            data: calldata.0,
            value: U256::zero(),
            gas_limit: 5000000,
        };
        let value = if commit {
            self.call(tx)?
        } else {
            self.staticcall(tx)?
        };
        let out = self.simulator.v2_simulate_swap_output(value.output)?;
        Ok(out)
    }

    pub fn get_amount_out(
        &mut self,
        amount_in: U256,
        reserve_in: U256,
        reserve_out: U256,
    ) -> Result<U256> {
        let calldata = self
            .simulator
            .get_amount_out_input(amount_in, reserve_in, reserve_out)?;
        let value = self.staticcall(Tx {
            caller: self.owner,
            transact_to: self.simulator_address,
            data: calldata.0,
            value: U256::zero(),
            gas_limit: 5000000,
        })?;
        let out = self.simulator.get_amount_out_output(value.output)?;
        Ok(out)
    }
}
