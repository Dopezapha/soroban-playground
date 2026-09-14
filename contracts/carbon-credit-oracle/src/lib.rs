#![no_std]
use soroban_sdk::{contract, contractimpl, symbol_short, Env, Symbol};

const VALUE: Symbol = symbol_short!("VALUE");

#[contract]
pub struct CarbonCreditOracle;

#[contractimpl]
impl CarbonCreditOracle {
    pub fn get_value(env: Env) -> u32 {
        env.storage().instance().get(&VALUE).unwrap_or_default()
    }

    pub fn set_value(env: Env, value: u32) {
        env.storage().instance().set(&VALUE, &value);
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn smoke() {
        assert_eq!(2 + 2, 4);
    }
}
