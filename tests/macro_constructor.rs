#[rulidity::contract]
mod constructor_contract {
    use rulidity::prelude::*;

    #[storage]
    struct ConstructorContract {
        owner: Address,
        count: U256,
    }

    impl ConstructorContract {
        #[constructor]
        fn construct(&mut self, owner: Address, start: U256) {
            self.owner = owner;
            self.count = start;
        }

        #[external]
        fn owner(&self) -> Address {
            self.owner
        }

        #[external]
        fn count(&self) -> U256 {
            self.count
        }

        #[external]
        #[allow(clippy::assign_op_pattern)]
        fn increment(&mut self) {
            self.count = self.count + U256::from(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use revm::context::TxEnv;
    use revm::context::result::ExecutionResult;
    use revm::database::{CacheDB, EmptyDB};
    use revm::primitives::{Address, Bytes, TxKind, U256};
    use revm::{Context, ExecuteCommitEvm, MainBuilder, MainContext};
    use rulidity::contract::Builder;

    fn addr_word(a: Address) -> [u8; 32] {
        let mut w = [0u8; 32];
        w[12..].copy_from_slice(a.as_slice());
        w
    }

    #[test]
    fn constructor_initializes_state() {
        let builder = Builder;
        let owner_sel = builder.selector("owner()");
        let count_sel = builder.selector("count()");
        let increment_sel = builder.selector("increment()");

        let caller = Address::from([0x11u8; 20]);
        let owner = Address::from([0xAAu8; 20]);
        let start: u64 = 41;

        // deploy data = creation code ++ abi-encoded constructor args (owner, start)
        let mut data = constructor_contract::deploy_code();
        data.extend_from_slice(&addr_word(owner));
        data.extend_from_slice(&U256::from(start).to_be_bytes::<32>());

        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();

        let deploy = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(caller)
                    .kind(TxKind::Create)
                    .data(Bytes::from(data))
                    .gas_limit(3_000_000)
                    .nonce(0)
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert!(deploy.is_success(), "deploy failed: {deploy:?}");
        let addr = deploy.created_address().unwrap();

        let mut send = |data: Bytes, nonce: u64| -> ExecutionResult {
            evm.transact_commit(
                TxEnv::builder()
                    .caller(caller)
                    .kind(TxKind::Call(addr))
                    .data(data)
                    .gas_limit(3_000_000)
                    .nonce(nonce)
                    .build()
                    .unwrap(),
            )
            .unwrap()
        };

        let read_addr = |r: ExecutionResult| -> Address {
            let out = r.into_output().unwrap();
            Address::from_slice(&out[12..32])
        };
        let read_u256 = |r: ExecutionResult| U256::from_be_slice(&r.into_output().unwrap());

        // both scalars set from the constructor args
        assert_eq!(read_addr(send(Bytes::from(owner_sel.to_vec()), 1)), owner);
        assert_eq!(
            read_u256(send(Bytes::from(count_sel.to_vec()), 2)),
            U256::from(start)
        );

        // constructor-initialized state is a normal starting point for mutation
        assert!(send(Bytes::from(increment_sel.to_vec()), 3).is_success());
        assert!(send(Bytes::from(increment_sel.to_vec()), 4).is_success());
        assert_eq!(
            read_u256(send(Bytes::from(count_sel.to_vec()), 5)),
            U256::from(start + 2)
        );
    }
}
