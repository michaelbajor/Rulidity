#[rulidity::contract]
mod branchy {
    use rulidity::prelude::*;

    #[storage]
    struct Branchy {}

    impl Branchy {
        #[external]
        #[allow(unused_assignments)]
        fn classify(&self, x: U256) -> U256 {
            let mut bucket = U256::from(4);
            if x < U256::from(10) {
                bucket = U256::from(1);
            } else if x < U256::from(20) {
                bucket = U256::from(2);
            } else if x < U256::from(30) {
                bucket = U256::from(3);
            } else {
                bucket = U256::from(4);
            }
            bucket
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

    #[test]
    fn else_if_chain_picks_the_right_branch() {
        let builder = Builder;
        let classify = builder.selector("classify(uint256)");
        let caller = Address::from([0x11u8; 20]);

        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();

        let deploy = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(caller)
                    .kind(TxKind::Create)
                    .data(Bytes::from(branchy::deploy_code()))
                    .gas_limit(3_000_000)
                    .nonce(0)
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert!(deploy.is_success(), "deploy failed: {deploy:?}");
        let addr = deploy.created_address().unwrap();

        let mut classify_of = {
            let mut nonce = 1u64;
            move |x: u64| -> U256 {
                let mut data = classify.to_vec();
                data.extend_from_slice(&U256::from(x).to_be_bytes::<32>());
                let r: ExecutionResult = evm
                    .transact_commit(
                        TxEnv::builder()
                            .caller(caller)
                            .kind(TxKind::Call(addr))
                            .data(Bytes::from(data))
                            .gas_limit(3_000_000)
                            .nonce(nonce)
                            .build()
                            .unwrap(),
                    )
                    .unwrap();
                nonce += 1;
                assert!(r.is_success(), "classify failed: {r:?}");
                U256::from_be_slice(&r.into_output().unwrap())
            }
        };

        assert_eq!(classify_of(0), U256::from(1));
        assert_eq!(classify_of(9), U256::from(1));

        assert_eq!(classify_of(10), U256::from(2));
        assert_eq!(classify_of(19), U256::from(2));

        assert_eq!(classify_of(20), U256::from(3));
        assert_eq!(classify_of(29), U256::from(3));

        assert_eq!(classify_of(30), U256::from(4));
        assert_eq!(classify_of(1000), U256::from(4));
    }
}
