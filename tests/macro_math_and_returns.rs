#[rulidity::contract]
mod mathy {
    use rulidity::prelude::*;

    #[storage]
    struct Mathy {
        a: U256,
        b: U256,
    }

    impl Mathy {
        #[constructor]
        fn construct(&mut self, a: U256, b: U256) {
            self.a = a;
            self.b = b;
        }

        #[external]
        fn sqrt(&self, y: U256) -> U256 {
            let mut z = y;
            let mut x = y / U256::from(2) + U256::from(1);
            while x < z {
                z = x;
                x = (y / x + x) / U256::from(2);
            }
            z
        }

        // div / mod / bit ops
        #[external]
        fn div(&self, x: U256, d: U256) -> U256 {
            x / d
        }

        #[external]
        fn rem(&self, x: U256, d: U256) -> U256 {
            x % d
        }

        #[external]
        fn shifted(&self, x: U256, n: U256) -> U256 {
            x << n
        }

        #[external]
        fn pair(&self) -> (U256, U256) {
            (self.a, self.b)
        }

        #[external]
        fn me(&self) -> Address {
            address_this()
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
    fn math_control_flow_and_returns() {
        let builder = Builder;
        let caller = Address::from([0x11u8; 20]);

        let mut data = mathy::deploy_code();
        data.extend_from_slice(&U256::from(11u64).to_be_bytes::<32>()); // a
        data.extend_from_slice(&U256::from(22u64).to_be_bytes::<32>()); // b

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

        let mut send = |data: Vec<u8>, nonce: u64| -> ExecutionResult {
            evm.transact_commit(
                TxEnv::builder()
                    .caller(caller)
                    .kind(TxKind::Call(addr))
                    .data(Bytes::from(data))
                    .gas_limit(3_000_000)
                    .nonce(nonce)
                    .build()
                    .unwrap(),
            )
            .unwrap()
        };

        let arg = |sel: [u8; 4], words: &[u64]| -> Vec<u8> {
            let mut d = sel.to_vec();
            for w in words {
                d.extend_from_slice(&U256::from(*w).to_be_bytes::<32>());
            }
            d
        };
        let word = |r: ExecutionResult| U256::from_be_slice(&r.into_output().unwrap());

        // sqrt: floor(sqrt(y)) via the while loop
        let sqrt = builder.selector("sqrt(uint256)");
        assert_eq!(word(send(arg(sqrt, &[144]), 1)), U256::from(12u64));
        assert_eq!(word(send(arg(sqrt, &[1_000_000]), 2)), U256::from(1000u64));
        assert_eq!(word(send(arg(sqrt, &[99]), 3)), U256::from(9u64)); // floor
        assert_eq!(word(send(arg(sqrt, &[0]), 4)), U256::from(0u64));
        assert_eq!(word(send(arg(sqrt, &[1]), 5)), U256::from(1u64));

        // div / rem / shift
        let div = builder.selector("div(uint256,uint256)");
        assert_eq!(word(send(arg(div, &[100, 7]), 6)), U256::from(14u64));
        let rem = builder.selector("rem(uint256,uint256)");
        assert_eq!(word(send(arg(rem, &[100, 7]), 7)), U256::from(2u64));
        let shifted = builder.selector("shifted(uint256,uint256)");
        assert_eq!(word(send(arg(shifted, &[1, 8]), 8)), U256::from(256u64));

        // multi-value return: (a, b) == (11, 22)
        let pair = builder.selector("pair()");
        let out = send(pair.to_vec(), 9).into_output().unwrap();
        assert_eq!(out.len(), 64);
        assert_eq!(U256::from_be_slice(&out[0..32]), U256::from(11u64));
        assert_eq!(U256::from_be_slice(&out[32..64]), U256::from(22u64));

        // address(this) == the contract's own address
        let me = builder.selector("me()");
        let out = send(me.to_vec(), 10).into_output().unwrap();
        assert_eq!(Address::from_slice(&out[12..32]), addr);
    }
}
