pub use alloy_primitives::{Address, U256};

pub struct Mapping<K, V> {
    _marker: core::marker::PhantomData<(K, V)>,
}

impl<K, V> Mapping<K, V> {
    pub fn get(&self, _key: K) -> V {
        unimplemented!(
            "Rulidity: storage exists only on-chain. This is stub impl for rust analyzer"
        )
    }

    pub fn insert(&mut self, _key: K, _value: V) {
        unimplemented!(
            "Rulidity: storage exists only on-chain. This is stub impl for rust analyzer"
        )
    }
}

pub struct Array<T> {
    _marker: core::marker::PhantomData<T>,
}

impl<T> Array<T> {
    pub fn get(&self, _index: U256) -> T {
        unimplemented!(
            "Rulidity: storage exists only on-chain. This is stub impl for rust analyzer"
        )
    }

    pub fn len(&self) -> U256 {
        unimplemented!(
            "Rulidity: storage exists only on-chain. This is stub impl for rust analyzer"
        )
    }

    pub fn push(&mut self, _value: T) {
        unimplemented!(
            "Rulidity: storage exists only on-chain. This is stub impl for rust analyzer"
        )
    }

    pub fn set(&mut self, _index: U256, _value: T) {
        unimplemented!(
            "Rulidity: storage exists only on-chain. This is stub impl for rust analyzer"
        )
    }
}

pub fn msg_sender() -> Address {
    unimplemented!("Rulidity: msg.sender exists only on-chain. This is stub impl for rust analyzer")
}

pub fn block_timestamp() -> U256 {
    unimplemented!("Rulidity: msg.sender exists only on-chain. This is stub impl for rust analyzer")
}

pub fn address_this() -> Address {
    unimplemented!(
        "Rulidity: address(this) exists only on-chain. This is stub impl for rust analyzer"
    )
}

pub fn require(_cond: bool) {
    unimplemented!("Rulidity: require executes onchain. This is stub for rust analyzer")
}

pub fn emit<E>(_event: E) {
    unimplemented!("Rulidity: emit works onchain. This is a stub for rust analyzer");
}

pub type ShortString = &'static str;
