#![no_std]
use soroban_sdk::{contract, contractimpl, symbol_short, Env, Symbol};

const SUBSCRIBERS: Symbol = symbol_short!("subscrb");

#[contract]
pub struct SubscriptionManager;

#[contractimpl]
impl SubscriptionManager {
    pub fn get_subscribers(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&SUBSCRIBERS)
            .unwrap_or_default()
    }

    pub fn set_subscribers(env: Env, count: u32) {
        env.storage().instance().set(&SUBSCRIBERS, &count);
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn smoke() {
        assert_eq!(2 + 2, 4);
    }
}
