use alloy_primitives::keccak256;

use crate::asm::{Asm, Op};

#[derive(Debug, Clone, Copy)]
pub struct Function {
    selector: [u8; 4],
    build_body: fn(&mut Asm), // this will append ops, must end in return/stop/revert
}

impl Function {
    pub fn new(selector: [u8; 4], build_body: fn(&mut Asm)) -> Self {
        Self {
            selector,
            build_body,
        }
    }
}

pub struct Builder;

impl Builder {
    pub fn selector(&self, sig: &str) -> [u8; 4] {
        let hash = keccak256(sig);
        hash[0..4]
            .try_into()
            .expect("keccak256 did not calculate hash")
    }

    fn build_runtime(&self, functions: Vec<Function>) -> Asm {
        let mut asm = Asm::new();

        // prologue: put 4-byte selector on stack
        asm.push_u8(0x00)
            .add_op(Op::CallDataLoad)
            .push_u8(0xE0)
            .add_op(Op::Shr);

        // allocate an entry label for each function, emit compare chain
        let mut entries = Vec::new();
        for func in functions {
            let label = asm.fresh_label();
            entries.push((func, label));

            asm.add_op(Op::Dup(1))
                .push_bytes(&func.selector)
                .add_op(Op::Eq)
                .add_op(Op::JumpI(label));
        }

        // if no selector matched, we revert @todo make it work with fallback and receive functions
        asm.revert_empty();

        // building each function's body
        for (func, label) in entries {
            asm.add_op(Op::JumpDest(label)).add_op(Op::Pop);
            (func.build_body)(&mut asm);
        }

        asm
    }

    fn wrap_with_constructor(&self, runtime_bytecode: Vec<u8>) -> Vec<u8> {
        let code_size = runtime_bytecode.len();

        debug_assert!(code_size <= 0xFFFF);

        let init_len = 15usize; // constant because size & offset use PUSH2
        let offset = init_len;

        let [size_hi, size_lo] = (code_size as u16).to_be_bytes();
        let [off_hi, off_lo] = (offset as u16).to_be_bytes();

        // codecopy(destOffest, offset, size) = push size, offset, dest(0)
        let mut init_code = vec![
            0x61, size_hi, size_lo, // PUSH2 size
            0x61, off_hi, off_lo, // PUSH2 offset
            0x60, 0x00, // PUSH1 0
            0x39, // CODECOPY -> mem[0..size] = runtime
            0x61, size_hi, size_lo, // PUSH2 size (for return)
            0x60, 0x00, // PUSH1 0
            0xF3, // RETURN
        ];

        init_code.extend_from_slice(&runtime_bytecode);
        debug_assert!(init_code.len() == init_len + code_size);

        init_code
    }

    pub fn assemble_contract(&self, functions: Vec<Function>) -> Vec<u8> {
        let runtime = self.build_runtime(functions).finish();
        self.wrap_with_constructor(runtime)
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::U256;
    use alloy_primitives::hex;

    use super::*;

    fn build_increment(asm: &mut Asm) {
        asm.load_slot(U256::from(0))
            .push_u8(1)
            .add_op(Op::Add)
            .store_slot(U256::from(0))
            .add_op(Op::Stop);
    }

    fn build_get(asm: &mut Asm) {
        asm.load_slot(U256::from(0)).return_word();
    }

    #[test]
    fn contract_assembles_deploys_and_works() {
        use revm::context::TxEnv;
        use revm::database::{CacheDB, EmptyDB};
        use revm::primitives::{Address, Bytes, TxKind, U256};
        use revm::{Context, ExecuteCommitEvm, MainBuilder, MainContext};

        let builder = Builder;

        let funcs = vec![
            Function {
                selector: builder.selector("increment()"),
                build_body: build_increment,
            },
            Function {
                selector: builder.selector("get()"),
                build_body: build_get,
            },
        ];

        let bytecode = builder.assemble_contract(funcs);

        let bytecode_bytes = bytecode.into();

        let caller = Address::from([0x10u8; 20]);
        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();

        let mut nonce = 0u64;

        let deploy = evm
            .transact_commit(
                TxEnv::builder()
                    .caller(caller)
                    .kind(TxKind::Create)
                    .data(bytecode_bytes)
                    .gas_limit(1_000_000)
                    .nonce(nonce)
                    .build()
                    .unwrap(),
            )
            .unwrap();

        assert!(deploy.is_success(), "deploy reverted: {deploy:?}");

        let contract_address = deploy.created_address().expect("no contract address");
        nonce += 1;

        let call = |evm: &mut _, function: &str, nonce: u64| {
            let sel = builder.selector(function);
            let r: revm::context::result::ExecutionResult = ExecuteCommitEvm::transact_commit(
                evm,
                TxEnv::builder()
                    .caller(caller)
                    .kind(TxKind::Call(contract_address))
                    .data(Bytes::from(sel.to_vec()))
                    .gas_limit(1_000_000)
                    .nonce(nonce)
                    .build()
                    .unwrap(),
            )
            .unwrap();
            assert!(r.is_success(), "call reverted: {r:?}");
            r
        };

        // 4. increment() twice
        call(&mut evm, "increment()", nonce);
        nonce += 1;
        call(&mut evm, "increment()", nonce);
        nonce += 1;

        // 5. get() and check it returns 2
        let r = call(&mut evm, "get()", nonce);
        let out = r.into_output().expect("no return data");
        assert_eq!(U256::from_be_slice(&out), U256::from(2));
    }

    #[test]
    fn calculating_selectors_works() {
        let function_name = "transfer(address,uint256)";
        let expected_signature1 = "a9059cbb";
        let function_name2 = "symbol()";
        let expected_signature2 = "95d89b41";

        let builder = Builder;

        let encoded1 = hex::encode(builder.selector(function_name));
        let encoded2 = hex::encode(builder.selector(function_name2));

        assert_eq!(expected_signature1, encoded1);
        assert_eq!(expected_signature2, encoded2);
    }
}
