#[rulidity::contract]
mod holders {
    use rulidity::prelude::*;

    #[storage]
    struct Holders {
        holders: Array<Address>,
    }

    impl Holders {
        #[external]
        fn add_holder(&mut self, addr: Address) {
            self.holders.push(addr);
        }

        #[external]
        fn count(&self) -> U256 {
            self.holders.len()
        }

        #[external]
        fn get(&self, idx: U256) -> Address {
            self.holders.get(idx)
        }

        #[external]
        fn set_holder(&mut self, idx: U256, addr: Address) {
            self.holders.set(idx, addr);
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
    fn dynamic_array_push_get_set_len() {
        let builder = Builder;
        let add_holder = builder.selector("add_holder(address)");
        let count = builder.selector("count()");
        let get = builder.selector("get(uint256)");
        let set_holder = builder.selector("set_holder(uint256,address)");

        let bytecode: Bytes = holders::deploy_code().into();

        let caller = Address::from([0x11u8; 20]);
        let alice = Address::from([0xAAu8; 20]);
        let bob = Address::from([0xBBu8; 20]);
        let carol = Address::from([0xCCu8; 20]);

        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();

        let deploy = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(caller)
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

        let word = |v: u64| U256::from(v).to_be_bytes::<32>();
        let add_call = |a: Address| -> Bytes {
            let mut d = add_holder.to_vec();
            d.extend_from_slice(&addr_word(a));
            Bytes::from(d)
        };
        let get_call = |i: u64| -> Bytes {
            let mut d = get.to_vec();
            d.extend_from_slice(&word(i));
            Bytes::from(d)
        };
        let set_call = |i: u64, a: Address| -> Bytes {
            let mut d = set_holder.to_vec();
            d.extend_from_slice(&word(i));
            d.extend_from_slice(&addr_word(a));
            Bytes::from(d)
        };
        let read_addr = |r: ExecutionResult| -> Address {
            let out = r.into_output().unwrap();
            Address::from_slice(&out[12..32])
        };
        let read_u256 = |r: ExecutionResult| U256::from_be_slice(&r.into_output().unwrap());

        // empty array, count == 0
        assert_eq!(
            read_u256(send(Bytes::from(count.to_vec()), 1)),
            U256::from(0)
        );

        // push two holders
        assert!(send(add_call(alice), 2).is_success());
        assert!(send(add_call(bob), 3).is_success());

        // len tracks the base slot
        assert_eq!(
            read_u256(send(Bytes::from(count.to_vec()), 4)),
            U256::from(2)
        );

        // elements land at keccak256(base) + i
        assert_eq!(read_addr(send(get_call(0), 5)), alice);
        assert_eq!(read_addr(send(get_call(1), 6)), bob);

        // overwrite index 0, length unchanged
        assert!(send(set_call(0, carol), 7).is_success());
        assert_eq!(read_addr(send(get_call(0), 8)), carol);
        assert_eq!(read_addr(send(get_call(1), 9)), bob);
        assert_eq!(
            read_u256(send(Bytes::from(count.to_vec()), 10)),
            U256::from(2)
        );
    }
}
