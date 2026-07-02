#[rulidity::contract]
mod token {
    use rulidity::prelude::*;

    #[event]
    struct Transfer {
        from: Address,
        to: Address,
        value: U256,
    }

    #[storage]
    struct Token {
        balances: Mapping<Address, U256>,
    }

    impl Token {
        #[external]
        fn balance_of(&self, who: Address) -> U256 {
            self.balances.get(who)
        }

        #[external]
        fn mint(&mut self, to: Address, amount: U256) {
            self.balances.insert(to, self.balances.get(to) + amount);
        }

        #[external]
        fn transfer(&mut self, to: Address, amount: U256) {
            let from = msg_sender();
            let from_bal = self.balances.get(from);
            require(from_bal >= amount);

            self.balances.insert(from, from_bal - amount);
            self.balances.insert(to, self.balances.get(to) + amount);

            emit(Transfer {
                from,
                to,
                value: amount,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use revm::context::TxEnv;
    use revm::context::result::ExecutionResult;
    use revm::database::{CacheDB, EmptyDB};
    use revm::primitives::{Address, Bytes, TxKind, U256, keccak256};
    use revm::{Context, ExecuteCommitEvm, MainBuilder, MainContext};
    use rulidity::contract::Builder;

    fn addr_word(a: Address) -> [u8; 32] {
        let mut w = [0u8; 32];
        w[12..].copy_from_slice(a.as_slice());
        w
    }

    #[test]
    fn transfer_emits_event() {
        let builder = Builder;
        let mint = builder.selector("mint(address,uint256)");
        let transfer = builder.selector("transfer(address,uint256)");
        let balance_of = builder.selector("balance_of(address)");

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

        // all calls come from alice; her nonce climbs by one each time
        // (a reverted-but-included tx still consumes its nonce).
        let mut send = |data: Bytes, nonce: u64| -> ExecutionResult {
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
        let addr_amount = |sel: [u8; 4], who: Address, amount: u64| -> Bytes {
            let mut d = sel.to_vec();
            d.extend_from_slice(&addr_word(who));
            d.extend_from_slice(&word(amount));
            Bytes::from(d)
        };
        let balance_call = |who: Address| -> Bytes {
            let mut d = balance_of.to_vec();
            d.extend_from_slice(&addr_word(who));
            Bytes::from(d)
        };
        let read_bal = |r: ExecutionResult| U256::from_be_slice(&r.into_output().unwrap());

        let m = send(addr_amount(mint, alice, 100), 1);
        assert!(m.is_success(), "mint failed: {m:?}");
        assert_eq!(m.logs().len(), 0, "mint emits no event");

        let r = send(addr_amount(transfer, bob, 30), 2);
        assert!(r.is_success(), "transfer failed: {r:?}");

        let logs = r.logs();
        assert_eq!(logs.len(), 1, "one Transfer event");
        let log = &logs[0];
        assert_eq!(log.address, addr, "emitted by the contract");

        // topic0 is always keccak(canonical signature)
        let topic0 = keccak256(b"Transfer(address,address,uint256)");
        assert_eq!(log.topics()[0], topic0, "topic0 = keccak(sig)");

        // there is only topic0, and all three fields sit in `data`.
        assert_eq!(
            log.topics().len(),
            1,
            "stage 1: only topic0 (no indexed yet)"
        );

        let data = &log.data.data;
        assert_eq!(data.len(), 96, "stage 1: from ++ to ++ value all in data");
        assert_eq!(&data[0..32], &addr_word(alice), "word 0 = from");
        assert_eq!(&data[32..64], &addr_word(bob), "word 1 = to");
        assert_eq!(
            U256::from_be_slice(&data[64..96]),
            U256::from(30),
            "word 2 = value"
        );

        assert_eq!(read_bal(send(balance_call(alice), 3)), U256::from(70));
        assert_eq!(read_bal(send(balance_call(bob), 4)), U256::from(30));

        let bad = send(addr_amount(transfer, bob, 1000), 5);
        assert!(!bad.is_success(), "over-balance transfer must revert");
        assert_eq!(bad.logs().len(), 0, "reverted tx emits no log");

        assert_eq!(read_bal(send(balance_call(alice), 6)), U256::from(70));
        assert_eq!(read_bal(send(balance_call(bob), 7)), U256::from(30));
    }
}
