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

    pub fn pop(&mut self) -> &mut Self {
        self.add_op(Op::Pop)
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

    pub fn push_topic(&mut self, sig: &str) -> &mut Self {
        let h = keccak256(sig.as_bytes());
        self.push_bytes(&h.0)
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
    fn msg_sender_works() {
        let mut asm = Asm::new();

        asm.msg_sender();

        let generated = hex::encode(asm.finish());
        assert_eq!(generated, "33");
    }
}
