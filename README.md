# Rulidity

A Rust eDSL that compiles to EVM bytecode. It is heavily inspired by Substrate's ink!.

The general idea is to have Rust compiler check for types and mutability and then have it compiled to EVM bytecode.

Another approach would be to create an LLVM target of EVM, but that would be a huuuugeeee pain due to architectural differences.

No-one asked for Rulidity and nowadays no-one really needs it. But I thought it might be a fun excercise to do.
