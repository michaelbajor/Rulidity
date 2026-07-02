use std::collections::HashMap;

use alloy_primitives::U256;

pub type Label = u32;

/// Generates the `Op` enum plus a `simple_byte()` lookup in one place, so an
/// opcode's variant name and its byte can never drift apart.
///
/// - `structured` variants carry operands/labels and are encoded by hand in `emit`.
/// - `simple` variants are zero-operand single-byte opcodes; their byte lives here.
macro_rules! define_ops {
    (
        structured { $($sname:ident($sty:ty)),* $(,)? }
        simple { $($name:ident = $byte:literal),* $(,)? }
    ) => {
        #[derive(Debug, Clone, PartialEq)]
        pub enum Op {
            $($sname($sty),)*
            $($name,)*
        }

        impl Op {
            /// Byte for zero-operand opcodes. `None` for structured ops
            /// (Push/Dup/Swap/Log/Jump/JumpI/JumpDest), which `emit` handles directly.
            fn simple_byte(&self) -> Option<u8> {
                match self {
                    $(Op::$name => Some($byte),)*
                    _ => None,
                }
            }
        }
    };
}

define_ops! {
    structured {
        Push(Vec<u8>),   // PUSH0..PUSH32, sized by operand len (empty = PUSH0)
        Dup(u8),         // DUP1..DUP16
        Swap(u8),        // SWAP1..SWAP16
        Log(u8),         // LOG0..LOG4
        Jump(Label),
        JumpI(Label),
        JumpDest(Label),
    }
    simple {
        Stop = 0x00, Add = 0x01, Mul = 0x02, Sub = 0x03, Div = 0x04, SDiv = 0x05,
        Mod = 0x06, SMod = 0x07, AddMod = 0x08, MulMod = 0x09, Exp = 0x0a, SignExtend = 0x0b,
        Lt = 0x10, Gt = 0x11, SLt = 0x12, SGt = 0x13, Eq = 0x14, IsZero = 0x15,
        And = 0x16, Or = 0x17, Xor = 0x18, Not = 0x19, Byte = 0x1a, Shl = 0x1b,
        Shr = 0x1c, Sar = 0x1d,
        Keccak256 = 0x20,
        Address = 0x30, Balance = 0x31, Origin = 0x32, Caller = 0x33, CallValue = 0x34,
        CallDataLoad = 0x35, CallDataSize = 0x36, CallDataCopy = 0x37, CodeSize = 0x38,
        CodeCopy = 0x39, GasPrice = 0x3a, ExtCodeSize = 0x3b, ExtCodeCopy = 0x3c,
        ReturnDataSize = 0x3d, ReturnDataCopy = 0x3e, ExtCodeHash = 0x3f,
        BlockHash = 0x40, Coinbase = 0x41, Timestamp = 0x42, Number = 0x43,
        PrevRandao = 0x44, GasLimit = 0x45, ChainId = 0x46, SelfBalance = 0x47,
        BaseFee = 0x48, BlobHash = 0x49, BlobBaseFee = 0x4a,
        Pop = 0x50, MLoad = 0x51, MStore = 0x52, MStore8 = 0x53, SLoad = 0x54,
        SStore = 0x55, Pc = 0x58, MSize = 0x59, Gas = 0x5a, TLoad = 0x5c, TStore = 0x5d,
        MCopy = 0x5e,
        Create = 0xf0, Call = 0xf1, CallCode = 0xf2, Return = 0xf3, DelegateCall = 0xf4,
        Create2 = 0xf5, StaticCall = 0xfa, Revert = 0xfd, Invalid = 0xfe, SelfDestruct = 0xff,
    }
}

impl Op {
    pub fn emit(&self, out: &mut Vec<u8>, lookup_table: &HashMap<Label, u32>) {
        match self {
            Op::Push(bytes) => {
                debug_assert!(bytes.len() <= 32);
                out.push(0x5F + bytes.len() as u8);
                out.extend_from_slice(bytes);
            }
            Op::Dup(n) => {
                debug_assert!((1..=16).contains(n));
                out.push(0x80 + (n - 1));
            }
            Op::Swap(n) => {
                debug_assert!((1..=16).contains(n));
                out.push(0x90 + (n - 1));
            }
            Op::Log(n) => {
                debug_assert!((0..=4).contains(n));
                out.push(0xA0 + n);
            }
            Op::Jump(label) => {
                // pass 1: label may be missing, value is irrelevant since the jump
                // is fixed-width (4 bytes), so PCs don't depend on it. pass 2: resolved.
                let pc = lookup_table.get(label).unwrap_or(&0);
                debug_assert!(*pc <= 0xFFFF);
                let [high_byte, low_byte] = (*pc as u16).to_be_bytes();
                out.push(0x61); // PUSH2
                out.push(high_byte);
                out.push(low_byte);
                out.push(0x56); // JUMP
            }
            Op::JumpI(label) => {
                let pc = lookup_table.get(label).unwrap_or(&0);
                debug_assert!(*pc <= 0xFFFF);
                let [high_byte, low_byte] = (*pc as u16).to_be_bytes();
                out.push(0x61); // PUSH2
                out.push(high_byte);
                out.push(low_byte);
                out.push(0x57); // JUMPI
            }
            Op::JumpDest(_) => out.push(0x5B),
            // every zero-operand opcode falls through to its table byte:
            _ => out.push(
                self.simple_byte()
                    .expect("structured ops are handled above; the rest must be simple"),
            ),
        }
    }
}

pub struct Asm {
    ops: Vec<Op>,
    next_label: Label,
}

impl Asm {
    pub fn new() -> Self {
        Self {
            ops: Vec::new(),
            next_label: 0,
        }
    }

    pub fn fresh_label(&mut self) -> Label {
        let label = self.next_label;
        self.next_label += 1;

        label
    }

    pub fn add_op(&mut self, op: Op) -> &mut Self {
        self.ops.push(op);
        self
    }

    pub fn push_u8(&mut self, byte: u8) -> &mut Self {
        self.add_op(Op::Push(vec![byte]));
        self
    }

    pub fn push_word(&mut self, word: U256) -> &mut Self {
        let bytes: [u8; 32] = word.to_be_bytes();

        let bytes_trimmed_zeros = bytes
            .iter()
            .skip_while(|&&byte| byte == 0)
            .map(|byte| *byte)
            .collect::<Vec<u8>>();

        self.add_op(Op::Push(bytes_trimmed_zeros));

        self
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.add_op(Op::Push(bytes.to_vec()))
    }

    /// Two-pass assembly: pass 1 fixes label PCs, pass 2 emits bytecode
    /// with correctly sized PUSH for jump targets
    pub fn finish(&self) -> Vec<u8> {
        let table = self.build_label_table();
        let mut out = Vec::new();

        for op in self.ops.iter() {
            op.emit(&mut out, &table);
        }

        out
    }

    fn build_label_table(&self) -> HashMap<Label, u32> {
        let mut table = HashMap::new();

        // scratch is kind of like a PC, but it's content does not matter, it's length is PC
        // otherwise there would need to be a function returning length of each opcode
        // instead, we can simply emit the actual bytes and the PC will be increasing by the length of emitted stuff
        let mut scratch_pad: Vec<u8> = Vec::new();

        for op in self.ops.iter() {
            if let Op::JumpDest(label) = op {
                table.insert(*label, scratch_pad.len() as u32);
            }

            op.emit(&mut scratch_pad, &table);
        }

        table
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use alloy_primitives::hex;

    #[test]
    fn linear_program_no_jumps() {
        let ops = vec![
            Op::Push([0x02].to_vec()),
            Op::Push([0x03].to_vec()),
            Op::Add,
        ];

        let asm = Asm { ops, next_label: 0 };
        let generated = asm.finish();
        let expected = vec![0x60, 0x02, 0x60, 0x03, 0x01];
        assert_eq!(generated, expected);
    }

    #[test]
    fn push0_and_multi_byte_push() {
        let ops = vec![Op::Push([].to_vec()), Op::Push([0xFF, 0x01].to_vec())];

        let asm = Asm { ops, next_label: 0 };
        let generated = asm.finish();
        let expected = vec![0x5F, 0x61, 0xFF, 0x01];
        assert_eq!(generated, expected);
    }

    #[test]
    fn dup_swap_pop() {
        let ops = vec![Op::Dup(1), Op::Swap(3), Op::Pop];

        let asm = Asm { ops, next_label: 0 };
        let generated = asm.finish();
        let expected = vec![0x80, 0x92, 0x50];
        assert_eq!(generated, expected);
    }

    #[test]
    fn forward_jump() {
        let ops = vec![Op::Jump(0), Op::Stop, Op::JumpDest(0), Op::Stop];

        let asm = Asm { ops, next_label: 0 };
        let generated = hex::encode(asm.finish());
        let expected = "61000556005b00";
        assert_eq!(generated, expected);
    }

    #[test]
    fn dispatcher_with_jumpi() {
        let ops = vec![
            Op::Push([0x00].to_vec()),
            Op::CallDataLoad,
            Op::Push([0xE0].to_vec()),
            Op::Shr,
            Op::Dup(1),
            Op::Push([0x12, 0x34, 0x56, 0x78].to_vec()),
            Op::Eq,
            Op::JumpI(0),
            Op::Revert,
            Op::JumpDest(0),
            Op::Stop,
        ];

        let asm = Asm { ops, next_label: 0 };
        let generated = hex::encode(asm.finish());
        let expected = "60003560e01c8063123456781461001257fd5b00";
        assert_eq!(generated, expected);
    }

    #[test]
    fn simple_opcodes_from_macro_table() {
        // a spread of the newly-added zero-operand opcodes, across byte ranges
        let ops = vec![Op::Mul, Op::IsZero, Op::Not, Op::Call, Op::SelfDestruct];
        let asm = Asm { ops, next_label: 0 };
        // MUL=02, ISZERO=15, NOT=19, CALL=f1, SELFDESTRUCT=ff
        assert_eq!(hex::encode(asm.finish()), "021519f1ff");
    }

    #[test]
    fn log_family() {
        let ops = vec![Op::Log(0), Op::Log(4)];
        let asm = Asm { ops, next_label: 0 };
        assert_eq!(hex::encode(asm.finish()), "a0a4");
    }
}
