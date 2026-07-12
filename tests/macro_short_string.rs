#[rulidity::contract]
mod token_meta {
    use rulidity::prelude::*;

    #[storage]
    struct Meta {
        symbol: ShortString,
    }

    impl Meta {
        #[constructor]
        fn construct(&mut self, symbol: ShortString) {
            self.symbol = symbol;
        }

        #[external]
        fn name(&self) -> ShortString {
            "MyToken"
        }

        #[external]
        fn symbol(&self) -> ShortString {
            self.symbol
        }

        #[external]
        fn set_symbol(&mut self, symbol: ShortString) {
            self.symbol = symbol;
        }
    }
}

// mirrors the ERC20 constructor shape: one static arg + two short strings
#[rulidity::contract]
mod erc20_like {
    use rulidity::prelude::*;

    #[storage]
    struct Meta {
        total_supply: U256,
        name: ShortString,
        symbol: ShortString,
    }

    impl Meta {
        #[constructor]
        fn construct(&mut self, total_supply: U256, name: ShortString, symbol: ShortString) {
            self.total_supply = total_supply;
            self.name = name;
            self.symbol = symbol;
        }

        #[external]
        fn totalSupply(&self) -> U256 {
            self.total_supply
        }

        #[external]
        fn name(&self) -> ShortString {
            self.name
        }

        #[external]
        fn symbol(&self) -> ShortString {
            self.symbol
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

    /// Decode a standard ABI-encoded `string` return: [offset(32)][len(32)][data..].
    fn read_string(r: ExecutionResult) -> String {
        let out = r.into_output().unwrap();
        let len = U256::from_be_slice(&out[32..64]).to::<usize>();
        String::from_utf8(out[64..64 + len].to_vec()).unwrap()
    }

    #[test]
    fn short_strings_return_as_abi() {
        let builder = Builder;
        let name_sel = builder.selector("name()");
        let symbol_sel = builder.selector("symbol()");

        let caller = Address::from([0x11u8; 20]);

        // deploy payload = creation code ++ abi.encode(symbol="TST")
        // (head: offset to the string; tail: length then padded data)
        let mut data = token_meta::deploy_code();
        data.extend_from_slice(&U256::from(0x20).to_be_bytes::<32>());
        data.extend_from_slice(&U256::from(3u64).to_be_bytes::<32>());
        let mut arg = [0u8; 32];
        arg[..3].copy_from_slice(b"TST");
        data.extend_from_slice(&arg);

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

        // pure literal return
        assert_eq!(
            read_string(send(Bytes::from(name_sel.to_vec()), 1)),
            "MyToken"
        );

        // constructor string arg: "TST" was decoded from the code tail into the slot
        assert_eq!(
            read_string(send(Bytes::from(symbol_sel.to_vec()), 2)),
            "TST"
        );

        // setter works
        let mut calldata = builder.selector("set_symbol(string)").to_vec();
        calldata.extend_from_slice(&U256::from(0x20).to_be_bytes::<32>());
        calldata.extend_from_slice(&U256::from(3u64).to_be_bytes::<32>());
        let mut data = [0u8; 32];
        data[..3].copy_from_slice(b"XYZ");
        calldata.extend_from_slice(&data);

        assert!(send(Bytes::from(calldata), 3).is_success());
        assert_eq!(
            read_string(send(Bytes::from(symbol_sel.to_vec()), 4)),
            "XYZ"
        );
    }

    /// Reproduces exactly what the Foundry `abi.encode(TOTAL_SUPPLY, NAME, SYMBOL)`
    /// blob looks like, to prove the (U256, String, String) constructor layout.
    #[test]
    fn erc20_style_constructor_args() {
        let builder = Builder;
        let supply_sel = builder.selector("totalSupply()");
        let name_sel = builder.selector("name()");
        let symbol_sel = builder.selector("symbol()");

        let caller = Address::from([0x11u8; 20]);
        let supply = U256::from(1_000_000u64);

        // abi.encode(total_supply, name, symbol):
        //   head: [supply][offset_name=0x60][offset_symbol=0xa0]
        //   tail: [name_len][name_data][symbol_len][symbol_data]
        let str_word = |s: &str| {
            let mut w = [0u8; 32];
            w[..s.len()].copy_from_slice(s.as_bytes());
            w
        };
        let mut data = erc20_like::deploy_code();
        data.extend_from_slice(&supply.to_be_bytes::<32>());
        data.extend_from_slice(&U256::from(0x60).to_be_bytes::<32>());
        data.extend_from_slice(&U256::from(0xa0).to_be_bytes::<32>());
        data.extend_from_slice(&U256::from(10u64).to_be_bytes::<32>());
        data.extend_from_slice(&str_word("TEST_TOKEN"));
        data.extend_from_slice(&U256::from(3u64).to_be_bytes::<32>());
        data.extend_from_slice(&str_word("TST"));

        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();

        let deploy = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(caller)
                    .kind(TxKind::Create)
                    .data(Bytes::from(data))
                    .gas_limit(5_000_000)
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

        assert_eq!(
            U256::from_be_slice(&send(Bytes::from(supply_sel.to_vec()), 1).into_output().unwrap()),
            supply
        );
        assert_eq!(read_string(send(Bytes::from(name_sel.to_vec()), 2)), "TEST_TOKEN");
        assert_eq!(read_string(send(Bytes::from(symbol_sel.to_vec()), 3)), "TST");
    }
}
