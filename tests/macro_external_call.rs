#[rulidity::contract]
mod callee {
    use rulidity::prelude::*;

    #[storage]
    struct Callee {
        v: U256,
    }

    impl Callee {
        #[constructor]
        fn construct(&mut self, v: U256) {
            self.v = v;
        }

        #[external]
        fn value(&self) -> U256 {
            self.v
        }

        #[external]
        fn set_value(&mut self, new_val: U256) {
            self.v = new_val;
        }
    }
}

#[rulidity::contract]
mod caller {
    use rulidity::prelude::*;

    #[interface]
    trait ICallee {
        fn value(&self) -> U256;
        fn set_value(&mut self, _new_val: U256);
    }

    #[storage]
    struct Caller {}

    impl Caller {
        #[external]
        fn read(&self, target: Address) -> U256 {
            ICallee::at(target).value()
        }

        #[external]
        fn set(&self, target: Address, val: U256) {
            ICallee::at(target).set_value(val);
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
    fn caller_reads_callee_via_staticcall() {
        let builder = Builder;
        let read_sel = builder.selector("read(address)");

        let deployer = Address::from([0x11u8; 20]);
        let value = U256::from(42u64);

        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();

        let mut deploy = |data: Vec<u8>, nonce: u64| -> Address {
            let r = evm
                .transact_commit(
                    TxEnv::builder()
                        .caller(deployer)
                        .kind(TxKind::Create)
                        .data(Bytes::from(data))
                        .gas_limit(5_000_000)
                        .nonce(nonce)
                        .build()
                        .unwrap(),
                )
                .unwrap();
            assert!(r.is_success(), "deploy failed: {r:?}");
            r.created_address().unwrap()
        };

        // callee: constructor takes v = 42 (single static arg appended to creation code)
        let mut callee_data = callee::deploy_code();
        callee_data.extend_from_slice(&value.to_be_bytes::<32>());
        let callee_addr = deploy(callee_data, 0);

        // caller: no constructor args
        let caller_addr = deploy(caller::deploy_code(), 1);

        // caller.read(callee_addr) -> STATICCALLs callee.value() and returns it
        let mut calldata = read_sel.to_vec();
        calldata.extend_from_slice(&addr_word(callee_addr));

        let out = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(deployer)
                    .kind(TxKind::Call(caller_addr))
                    .data(Bytes::from(calldata))
                    .gas_limit(3_000_000)
                    .nonce(2)
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert!(out.is_success(), "read failed: {out:?}");

        let read_u256 = |r: ExecutionResult| U256::from_be_slice(&r.into_output().unwrap());
        assert_eq!(read_u256(out), value);
    }

    #[test]
    fn caller_writes_callee_via_call() {
        let builder = Builder;
        let set_sel = builder.selector("set(address,uint256)");
        let read_sel = builder.selector("read(address)");

        let deployer = Address::from([0x11u8; 20]);
        let initial = U256::from(42u64);
        let updated = U256::from(99u64);

        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();

        let mut deploy = |data: Vec<u8>, nonce: u64| -> Address {
            let r = evm
                .transact_commit(
                    TxEnv::builder()
                        .caller(deployer)
                        .kind(TxKind::Create)
                        .data(Bytes::from(data))
                        .gas_limit(5_000_000)
                        .nonce(nonce)
                        .build()
                        .unwrap(),
                )
                .unwrap();
            assert!(r.is_success(), "deploy failed: {r:?}");
            r.created_address().unwrap()
        };

        let mut callee_data = callee::deploy_code();
        callee_data.extend_from_slice(&initial.to_be_bytes::<32>());
        let callee_addr = deploy(callee_data, 0);
        let caller_addr = deploy(caller::deploy_code(), 1);

        let mut call = |data: Vec<u8>, nonce: u64| -> ExecutionResult {
            evm.transact_commit(
                TxEnv::builder()
                    .caller(deployer)
                    .kind(TxKind::Call(caller_addr))
                    .data(Bytes::from(data))
                    .gas_limit(3_000_000)
                    .nonce(nonce)
                    .build()
                    .unwrap(),
            )
            .unwrap()
        };

        let mut set_data = set_sel.to_vec();
        set_data.extend_from_slice(&addr_word(callee_addr));
        set_data.extend_from_slice(&updated.to_be_bytes::<32>());
        assert!(call(set_data, 2).is_success(), "set failed");

        let mut read_data = read_sel.to_vec();
        read_data.extend_from_slice(&addr_word(callee_addr));
        let out = call(read_data, 3);
        assert!(out.is_success(), "read failed: {out:?}");
        assert_eq!(U256::from_be_slice(&out.into_output().unwrap()), updated);
    }
}
