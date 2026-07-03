#[rulidity::contract]
mod erc20 {
    use rulidity::prelude::*;

    #[event]
    struct Transfer {
        #[indexed]
        _from: Address,
        #[indexed]
        _to: Address,
        _value: U256,
    }

    #[event]
    struct Approval {
        #[indexed]
        _owner: Address,
        #[indexed]
        _spender: Address,
        _value: U256,
    }

    #[storage]
    struct ERC20 {
        balances: Mapping<Address, U256>,
        allowances: Mapping<Address, Mapping<Address, U256>>,
        total_supply: U256,
    }

    impl ERC20 {
        #[constructor]
        fn construct(&mut self, total_supply: U256) {
            self.total_supply = total_supply;
            self.balances.insert(msg_sender(), total_supply);
        }

        #[external]
        fn totalSupply(&self) -> U256 {
            self.total_supply
        }

        #[external]
        fn balanceOf(&self, who: Address) -> U256 {
            self.balances.get(who)
        }

        #[external]
        fn allowance(&self, owner: Address, spender: Address) -> U256 {
            self.allowances.get(owner).get(spender)
        }

        #[external]
        fn transfer(&mut self, to: Address, amount: U256) {
            let from = msg_sender();
            self._transfer(from, to, amount);
        }

        #[external]
        fn transferFrom(&mut self, from: Address, to: Address, amount: U256) {
            let spender = msg_sender();
            self._spend_allowance(from, spender, amount);
            self._transfer(from, to, amount);
        }

        #[external]
        fn approve(&mut self, spender: Address, value: U256) {
            let owner = msg_sender();
            self._approve(owner, spender, value, true);
        }

        fn _transfer(&mut self, from: Address, to: Address, amount: U256) {
            require(self.balances.get(from) >= amount);

            self.balances.insert(from, self.balances.get(from) - amount);
            self.balances.insert(to, self.balances.get(to) + amount);

            emit(Transfer {
                _from: from,
                _to: to,
                _value: amount,
            });
        }

        fn _spend_allowance(&mut self, owner: Address, spender: Address, value: U256) {
            let current_allowance = self.allowances.get(owner).get(spender);
            require(current_allowance >= value);

            self._approve(owner, spender, current_allowance - value, false);
        }

        fn _approve(&mut self, owner: Address, spender: Address, value: U256, emit_event: bool) {
            self.allowances.get(owner).insert(spender, value);

            if emit_event {
                emit(Approval {
                    _owner: owner,
                    _spender: spender,
                    _value: value,
                });
            }
        }
    }
}

fn main() {
    let bytecode = alloy_primitives::hex::encode(erc20::deploy_code());
    let abi = erc20::abi_json();

    std::fs::write("ERC20.bin", format!("0x{}", bytecode)).unwrap();
    std::fs::write("ERC20.abi.json", abi).unwrap();
}
