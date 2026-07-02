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
                self.balances.get(msg_sender()) + U256::from(100),
            );
        }

        #[external]
        fn balance_of(&self, who: Address) -> U256 {
            self.balances.get(who)
        }

        #[external]
        fn transfer(&mut self, to: Address, amount: U256) {
            let from_bal = self.balances.get(msg_sender());
            require(from_bal >= amount);
            self.balances.insert(msg_sender(), from_bal - amount);
            self.balances.insert(to, self.balances.get(to) + amount);
        }

        #[external]
        fn capped_deposit(&mut self, amount: U256) {
            if amount < U256::from(1000) {
                self.balances
                    .insert(msg_sender(), self.balances.get(msg_sender()) + amount);
            }
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

    fn decode_u256(r: ExecutionResult) -> U256 {
        U256::from_be_slice(&r.into_output().unwrap())
    }

    #[test]
    fn require_and_if_work() {
        let builder = Builder;
        let deposit = builder.selector("deposit()");
        let balance_of = builder.selector("balance_of(address)");
        let transfer = builder.selector("transfer(address,uint256)");
        let capped_deposit = builder.selector("capped_deposit(uint256)");

        let bytecode: Bytes = token::deploy_code().into();

        let alice = Address::from([0xAAu8; 20]);
        let bob = Address::from([0xBBu8; 20]);

        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();

        let deploy = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(alice)
                    .kind(TxKind::Create)
                    .data(bytecode)
                    .gas_limit(3_000_000)
                    .nonce(0)
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert!(deploy.is_success(), "deploy failed: {deploy:?}");
        let addr = deploy.created_address().unwrap();

        // one closure for every call; alice's nonce climbs by one each time
        // (a reverted-but-included tx still consumes its nonce).
        let mut send = |data: Bytes, nonce: u64| {
            evm.transact_commit(
                TxEnv::builder()
                    .caller(alice)
                    .kind(TxKind::Call(addr))
                    .data(data)
                    .gas_limit(3_000_000)
                    .nonce(nonce)
                    .build()
                    .unwrap(),
            )
            .unwrap()
        };

        let word = |v: u64| U256::from(v).to_be_bytes::<32>();
        let balance_call = |who: Address| -> Bytes {
            let mut d = balance_of.to_vec();
            d.extend_from_slice(&addr_word(who));
            Bytes::from(d)
        };
        let transfer_call = |to: Address, amount: u64| -> Bytes {
            let mut d = transfer.to_vec();
            d.extend_from_slice(&addr_word(to));
            d.extend_from_slice(&word(amount));
            Bytes::from(d)
        };
        let capped_call = |amount: u64| -> Bytes {
            let mut d = capped_deposit.to_vec();
            d.extend_from_slice(&word(amount));
            Bytes::from(d)
        };

        assert!(send(Bytes::from(deposit.to_vec()), 1).is_success());

        assert!(
            send(transfer_call(bob, 30), 2).is_success(),
            "transfer within balance should succeed"
        );
        assert_eq!(decode_u256(send(balance_call(alice), 3)), U256::from(70));
        assert_eq!(decode_u256(send(balance_call(bob), 4)), U256::from(30));

        let r = send(transfer_call(bob, 1000), 5);
        assert!(!r.is_success(), "over-balance transfer must revert: {r:?}");
        assert_eq!(decode_u256(send(balance_call(alice), 6)), U256::from(70));
        assert_eq!(decode_u256(send(balance_call(bob), 7)), U256::from(30));

        assert!(send(capped_call(50), 8).is_success());
        assert_eq!(decode_u256(send(balance_call(alice), 9)), U256::from(120));

        assert!(send(capped_call(5000), 10).is_success());
        assert_eq!(
            decode_u256(send(balance_call(alice), 11)),
            U256::from(120),
            "branch skipped, balance unchanged"
        );
    }
}
