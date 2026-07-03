#[rulidity::contract]
mod counter {
    use rulidity::prelude::*;

    #[storage]
    struct Counter {
        count: U256,
    }

    impl Counter {
        #[external]
        #[allow(clippy::assign_op_pattern)] // this is deliberate here to make sure macro works on those cases as well
        fn increment(&mut self) {
            self.count = self.count + U256::from(1);
        }

        #[external]
        fn get(&self) -> U256 {
            self.count
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use revm::context::TxEnv;
    use revm::database::{CacheDB, EmptyDB};
    use revm::primitives::{Address, Bytes, TxKind, U256};
    use revm::{Context, ExecuteCommitEvm, MainBuilder, MainContext};
    use rulidity::contract::Builder;

    #[test]
    fn macro_counter_works() {
        let builder = Builder;
        let inc = builder.selector("increment()");
        let get = builder.selector("get()");

        let bytecode: Bytes = counter::deploy_code().into();

        let caller = Address::from([0x11u8; 20]);
        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();
        let mut nonce = 0u64;

        let deploy = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(caller)
                    .kind(TxKind::Create)
                    .data(bytecode)
                    .gas_limit(1_000_000)
                    .nonce(nonce)
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert!(deploy.is_success(), "deploy failed: {deploy:?}");
        let addr = deploy.created_address().unwrap();
        nonce += 1;

        for _ in 0..2 {
            let r = evm
                .transact_commit(
                    TxEnv::builder()
                        .caller(caller)
                        .kind(TxKind::Call(addr))
                        .data(Bytes::from(inc.to_vec()))
                        .gas_limit(1_000_000)
                        .nonce(nonce)
                        .build()
                        .unwrap(),
                )
                .unwrap();
            assert!(r.is_success(), "increment failed: {r:?}");
            nonce += 1;
        }

        let r = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(caller)
                    .kind(TxKind::Call(addr))
                    .data(Bytes::from(get.to_vec()))
                    .gas_limit(1_000_000)
                    .nonce(nonce)
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert!(r.is_success(), "get failed: {r:?}");
        let out = r.into_output().unwrap();
        assert_eq!(U256::from_be_slice(&out), U256::from(2));
    }
}
