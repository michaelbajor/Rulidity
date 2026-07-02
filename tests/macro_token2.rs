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

        #[external]
        fn mint(&mut self, to: Address, amount: U256) {
            let current = self.balances.get(to);
            self.balances.insert(to, current + amount);
        }

        #[external]
        fn balance_of(&self, who: Address) -> U256 {
            self.balances.get(who)
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

    /// ABI-encode an address as a 32-byte word (left-padded; address in the low 20 bytes).
    fn addr_word(a: Address) -> [u8; 32] {
        let mut w = [0u8; 32];
        w[12..].copy_from_slice(a.as_slice());
        w
    }

    #[test]
    fn function_parameters_work() {
        let builder = Builder;
        let mint = builder.selector("mint(address,uint256)");
        let balance_of = builder.selector("balance_of(address)");

        let bytecode: Bytes = token::deploy_code().into();

        let caller = Address::from([0x11u8; 20]);
        let alice = Address::from([0xAAu8; 20]); // note: NOT the caller
        let bob = Address::from([0xBBu8; 20]);

        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();

        let deploy = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(caller)
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

        // build calldata: selector ++ 32-byte-per-arg
        let mint_call = |to: Address, amount: u64| -> Bytes {
            let mut data = mint.to_vec();
            data.extend_from_slice(&addr_word(to));
            data.extend_from_slice(&U256::from(amount).to_be_bytes::<32>());
            Bytes::from(data)
        };
        let balance_call = |who: Address| -> Bytes {
            let mut data = balance_of.to_vec();
            data.extend_from_slice(&addr_word(who));
            Bytes::from(data)
        };

        let mut send = |data: Bytes, nonce: u64| {
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

        assert!(
            send(mint_call(alice, 100), 1).is_success(),
            "mint #1 failed"
        );
        assert!(send(mint_call(alice, 50), 2).is_success(), "mint #2 failed");

        let r = send(balance_call(alice), 3);
        assert!(r.is_success(), "balance_of(alice) failed: {r:?}");
        assert_eq!(
            U256::from_be_slice(&r.into_output().unwrap()),
            U256::from(150),
            "alice should have 150"
        );

        // bob was never minted to, so 0: proves the param really keys the mapping
        let r = send(balance_call(bob), 4);
        assert!(r.is_success(), "balance_of(bob) failed: {r:?}");
        assert_eq!(
            U256::from_be_slice(&r.into_output().unwrap()),
            U256::from(0),
            "bob should have 0"
        );
    }
}
