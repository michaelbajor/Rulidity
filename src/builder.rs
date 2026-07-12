use alloy_primitives::{U256, keccak256};

use crate::asm::{Asm, Op};

impl Asm {
    pub fn sload(&mut self) -> &mut Self {
        self.add_op(Op::SLoad)
    }

    pub fn sstore(&mut self) -> &mut Self {
        self.add_op(Op::SStore)
    }

    pub fn mload(&mut self) -> &mut Self {
        self.add_op(Op::MLoad)
    }

    pub fn mstore(&mut self) -> &mut Self {
        self.add_op(Op::MStore)
    }

    pub fn load_slot(&mut self, slot: U256) -> &mut Self {
        self.push_word(slot).sload()
    }

    pub fn store_slot(&mut self, slot: U256) -> &mut Self {
        self.push_word(slot).sstore()
    }

    pub fn add(&mut self) -> &mut Self {
        self.add_op(Op::Add)
    }

    pub fn keccak256(&mut self) -> &mut Self {
        self.add_op(Op::Keccak256)
    }

    pub fn return_word(&mut self) -> &mut Self {
        self.push_u8(0x00) // stack: [value, 0]
            .add_op(Op::MStore) // MSTORE pops 0-offset then value -> mem[0..32]=value, stack = []
            .push_u8(0x20) // [0x20] (for length = 32)
            .push_u8(0x00) // [0x20, 0x00]  (offset = 0, on top)
            .add_op(Op::Return); // Return pops offset then length
        self
    }

    pub fn revert_empty(&mut self) -> &mut Self {
        self.push_u8(0x00).push_u8(0x00).add_op(Op::Revert)
    }

    pub fn mapping_slot(&mut self, base: U256) -> &mut Self {
        self.push_u8(0x00)
            .add_op(Op::MStore)
            .push_word(base)
            .push_u8(0x20)
            .add_op(Op::MStore)
            .push_u8(0x40)
            .push_u8(0x00)
            .add_op(Op::Keccak256)
    }

    pub fn array_elem_slot(&mut self, base: U256) -> &mut Self {
        self.push_word(base)
            .push_u8(0x00)
            .mstore()
            .push_u8(0x20)
            .push_u8(0x00)
            .keccak256()
            .add()
    }

    /// keccak256(key ++ base) with base and key both taken from the stack, so
    /// the base can itself be a computed slot (nested mappings).
    /// stack: [base, key] -> [slot]
    pub fn mapping_slot_from_stack(&mut self) -> &mut Self {
        self.push_u8(0x00)
            .mstore() // mem[0..32] = key
            .push_u8(0x20)
            .mstore() // mem[32..64] = base
            .push_u8(0x40)
            .push_u8(0x00)
            .keccak256()
    }

    /// keccak256(base) + index with base and index both taken from the stack.
    /// stack: [base, index] -> [slot]
    pub fn array_elem_slot_from_stack(&mut self) -> &mut Self {
        self.add_op(Op::Swap(1)) // [index, base]
            .push_u8(0x00)
            .mstore() // mem[0..32] = base
            .push_u8(0x20)
            .push_u8(0x00)
            .keccak256() // [index, keccak(base)]
            .add()
    }

    pub fn pop(&mut self) -> &mut Self {
        self.add_op(Op::Pop)
    }

    pub fn and(&mut self) -> &mut Self {
        self.add_op(Op::And)
    }

    pub fn or(&mut self) -> &mut Self {
        self.add_op(Op::Or)
    }

    pub fn not(&mut self) -> &mut Self {
        self.add_op(Op::Not)
    }

    pub fn dup1(&mut self) -> &mut Self {
        self.add_op(Op::Dup(1))
    }

    pub fn swap1(&mut self) -> &mut Self {
        self.add_op(Op::Swap(1))
    }

    pub fn shr(&mut self) -> &mut Self {
        self.add_op(Op::Shr)
    }

    pub fn shl(&mut self) -> &mut Self {
        self.add_op(Op::Shl)
    }

    pub fn ret(&mut self) -> &mut Self {
        self.add_op(Op::Return)
    }

    pub fn code_size(&mut self) -> &mut Self {
        self.add_op(Op::CodeSize)
    }

    pub fn code_copy(&mut self) -> &mut Self {
        self.add_op(Op::CodeCopy)
    }

    pub fn sub(&mut self) -> &mut Self {
        self.add_op(Op::Sub)
    }

    pub fn msg_sender(&mut self) -> &mut Self {
        self.add_op(Op::Caller)
    }

    pub fn tx_origin(&mut self) -> &mut Self {
        self.add_op(Op::Origin)
    }

    pub fn balance(&mut self) -> &mut Self {
        self.add_op(Op::Balance)
    }

    pub fn calldataload(&mut self) -> &mut Self {
        self.add_op(Op::CallDataLoad)
    }

    pub fn push_topic(&mut self, sig: &str) -> &mut Self {
        let h = keccak256(sig.as_bytes());
        self.push_bytes(&h.0)
    }

    pub fn return_short_string(&mut self) -> &mut Self {
        self.dup1()
            .push_u8(0xff)
            .and()
            .push_u8(0x1)
            .shr()
            .push_u8(0x20)
            .mstore()
            .push_u8(0xff)
            .not()
            .and()
            .push_u8(0x40)
            .mstore()
            .push_u8(0x20)
            .push_u8(0x00)
            .mstore()
            .push_u8(0x60)
            .push_u8(0x00)
            .ret()
    }

    pub fn decode_short_string_param(&mut self, head: u32, slot: u32) -> &mut Self {
        let ok = self.fresh_label();
        self.push_word(U256::from(head))
            .calldataload() // [rel_offset]
            .push_u8(0x04)
            .add() // [abs] -> points at the length word
            .dup1()
            .calldataload() // [abs, len]
            // guard: revert unless len < 32, else the length byte we fold in would clash
            .dup1() // [abs, len, len]
            .push_u8(32) // [abs, len, len, 32]
            .add_op(Op::Gt) // [abs, len, (32 > len) = (len < 32)]
            .add_op(Op::JumpI(ok)) // consume bool; skip the revert if len < 32
            .revert_empty() // [abs, len] (only reached when len >= 32)
            .add_op(Op::JumpDest(ok)) // [abs, len]
            .swap1() // [len, abs]
            .push_u8(0x20)
            .add()
            .calldataload() // [len, data] (data word at abs + 0x20)
            .push_u8(0xff)
            .not()
            .and() // [len, data & ~0xff]
            .swap1() // [data & ~0xff, len]
            .push_u8(0x01)
            .shl() // [data & ~0xff, len << 1]
            .or() // [packed]
            .push_word(U256::from(slot))
            .mstore() // mem[slot] = packed
    }

    pub fn decode_short_string_constructor_arg(
        &mut self,
        len_back: u32,
        data_back: u32,
        slot: u32,
    ) -> &mut Self {
        let ok = self.fresh_label();

        self.push_u8(0x20) // copy 32 bytes
            .push_word(U256::from(len_back))
            .code_size()
            .sub() // src = CODESIZE - len_back
            .push_u8(0x00)
            .code_copy() // mem[0x00] = len word
            .push_u8(0x20)
            .push_word(U256::from(data_back))
            .code_size()
            .sub() // src = CODESIZE - data_back
            .push_u8(0x20)
            .code_copy(); // mem[0x20] = data word

        // gaurd to revert unless len < 32
        self.push_u8(0x00)
            .mload() // [len]
            .dup1()
            .push_u8(32)
            .add_op(Op::Gt) // [len, (32 > len) = (len < 32)]
            .add_op(Op::JumpI(ok))
            .revert_empty()
            .add_op(Op::JumpDest(ok));

        // pack: mem[0x20] & ~0xff | (len << 1)
        self.push_u8(0x01)
            .shl() // [len << 1]
            .push_u8(0x20)
            .mload() // [len << 1, data]
            .push_u8(0xff)
            .not()
            .and() // [len << 1, data & ~0xff]
            .or() // [packed]
            .push_word(U256::from(slot))
            .mstore() // mem[slot] = packed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;

    #[test]
    fn load_slot_works() {
        let mut asm = Asm::new();

        asm.load_slot(U256::from(0));

        let generated = hex::encode(asm.finish());
        assert_eq!(generated, "5f54");
    }

    #[test]
    fn load_slot_works2() {
        let mut asm = Asm::new();

        asm.load_slot(U256::from(5));

        let generated = hex::encode(asm.finish());
        assert_eq!(generated, "600554");
    }

    #[test]
    fn store_slot_works() {
        let mut asm = Asm::new();

        asm.store_slot(U256::from(0));

        let generated = hex::encode(asm.finish());
        assert_eq!(generated, "5f55");
    }

    #[test]
    fn return_word_works() {
        let mut asm = Asm::new();

        asm.return_word();

        let generated = hex::encode(asm.finish());
        assert_eq!(generated, "60005260206000f3");
    }

    #[test]
    fn revert_empty_works() {
        let mut asm = Asm::new();

        asm.revert_empty();

        let generated = hex::encode(asm.finish());
        assert_eq!(generated, "60006000fd");
    }

    #[test]
    fn mapping_slot_works() {
        let mut asm = Asm::new();

        asm.mapping_slot(U256::from(1));

        let generated = hex::encode(asm.finish());
        assert_eq!(generated, "60005260016020526040600020");
    }

    #[test]
    fn mapping_slot_from_stack_works() {
        let mut asm = Asm::new();

        asm.mapping_slot_from_stack();

        let generated = hex::encode(asm.finish());
        assert_eq!(generated, "6000526020526040600020");
    }

    #[test]
    fn array_elem_slot_from_stack_works() {
        let mut asm = Asm::new();

        asm.array_elem_slot_from_stack();

        let generated = hex::encode(asm.finish());
        assert_eq!(generated, "90600052602060002001");
    }

    #[test]
    fn msg_sender_works() {
        let mut asm = Asm::new();

        asm.msg_sender();

        let generated = hex::encode(asm.finish());
        assert_eq!(generated, "33");
    }
}
