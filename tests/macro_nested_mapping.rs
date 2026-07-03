#[rulidity::contract]
mod allowance {
    use rulidity::prelude::*;

    #[storage]
    struct Allowance {
        allowances: Mapping<Address, Mapping<Address, U256>>,
    }

    impl Allowance {
        #[external]
        fn approve(&mut self, spender: Address, amount: U256) {
            self.allowances.get(msg_sender()).insert(spender, amount);
        }

        #[external]
        fn allowance(&self, owner: Address, spender: Address) -> U256 {
            self.allowances.get(owner).get(spender)
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
    fn nested_mapping_approve_and_read() {
        let builder = Builder;
        let approve = builder.selector("approve(address,uint256)");
        let allowance = builder.selector("allowance(address,address)");

        let owner = Address::from([0x11u8; 20]); // caller -> msg_sender()
        let spender = Address::from([0xAAu8; 20]);
        let other = Address::from([0xBBu8; 20]);
        let amount: u64 = 4_242;

        let bytecode: Bytes = allowance::deploy_code().into();

        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();

        let deploy = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(owner)
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

        let mut send = |data: Bytes, nonce: u64| -> ExecutionResult {
            evm.transact_commit(
                TxEnv::builder()
                    .caller(owner)
                    .kind(TxKind::Call(addr))
                    .data(data)
                    .gas_limit(3_000_000)
                    .nonce(nonce)
                    .build()
                    .unwrap(),
            )
            .unwrap()
        };

        let approve_call = |spender: Address, amount: u64| -> Bytes {
            let mut d = approve.to_vec();
            d.extend_from_slice(&addr_word(spender));
            d.extend_from_slice(&U256::from(amount).to_be_bytes::<32>());
            Bytes::from(d)
        };
        let allowance_call = |owner: Address, spender: Address| -> Bytes {
            let mut d = allowance.to_vec();
            d.extend_from_slice(&addr_word(owner));
            d.extend_from_slice(&addr_word(spender));
            Bytes::from(d)
        };
        let read_u256 = |r: ExecutionResult| U256::from_be_slice(&r.into_output().unwrap());

        // owner (msg_sender) approves spender for `amount`
        assert!(send(approve_call(spender, amount), 1).is_success());

        // allowances[owner][spender] == amount
        assert_eq!(
            read_u256(send(allowance_call(owner, spender), 2)),
            U256::from(amount)
        );

        // a different inner key is untouched: allowances[owner][other] == 0
        assert_eq!(
            read_u256(send(allowance_call(owner, other), 3)),
            U256::from(0)
        );

        // a different outer key is untouched: allowances[other][spender] == 0
        assert_eq!(
            read_u256(send(allowance_call(other, spender), 4)),
            U256::from(0)
        );
    }
}
