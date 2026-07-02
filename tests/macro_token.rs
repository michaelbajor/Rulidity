#[rulidity::contract]
mod token {
    use rulidity::prelude::*;

    #[storage]
    struct Token {
        balances: Mapping<Address, U256>,
    }

    impl Token {
        #[external]
        fn deposit(&mut self) {
            self.balances.insert(
                msg_sender(),
                self.balances.get(msg_sender()) + U256::from(1),
            );
        }

        #[external]
        fn my_balance(&self) -> U256 {
            self.balances.get(msg_sender())
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
    fn mapping_and_msg_sender_work() {
        let builder = Builder;
        let deposit = builder.selector("deposit()");
        let my_balance = builder.selector("my_balance()");

        let bytecode: Bytes = token::deploy_code().into();

        let alice = Address::from([0x11u8; 20]);
        let bob = Address::from([0x22u8; 20]);

        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();

        let deploy = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(alice)
                    .kind(TxKind::Create)
                    .data(bytecode)
                    .gas_limit(2_000_000)
                    .nonce(0)
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert!(deploy.is_success(), "deploy failed: {deploy:?}");
        let addr = deploy.created_address().unwrap();

        let mut send = |caller: Address, data: Bytes, nonce: u64| {
            evm.transact_commit(
                TxEnv::builder()
                    .caller(caller)
                    .kind(TxKind::Call(addr))
                    .data(data)
                    .gas_limit(2_000_000)
                    .nonce(nonce)
                    .build()
                    .unwrap(),
            )
            .unwrap()
        };

        assert!(send(alice, Bytes::from(deposit.to_vec()), 1).is_success());
        assert!(send(alice, Bytes::from(deposit.to_vec()), 2).is_success());

        let ra = send(alice, Bytes::from(my_balance.to_vec()), 3);
        assert!(ra.is_success(), "my_balance failed: {ra:?}");
        assert_eq!(
            U256::from_be_slice(&ra.into_output().unwrap()),
            U256::from(2),
            "alice should have 2"
        );

        // bob never deposited, so his balance is 0 (mapping keys on the caller)
        let rb = send(bob, Bytes::from(my_balance.to_vec()), 0);
        assert!(rb.is_success(), "my_balance failed: {rb:?}");
        assert_eq!(
            U256::from_be_slice(&rb.into_output().unwrap()),
            U256::from(0),
            "bob should have 0"
        );
    }
}
