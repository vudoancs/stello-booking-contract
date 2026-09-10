//! Minimal token for atomicity tests: can refuse transfers to a chosen recipient.
//!
//! Classic trustline failures are not faithfully reproduced by SAC testutils.
//! This contract is the closest supported cross-contract failure injection.
#![cfg(test)]

use soroban_sdk::{Address, Env, contract, contractimpl, contracttype};

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Balance(Address),
    FailTo,
    FailArmed,
}

#[contract]
pub struct ScriptedToken;

#[contractimpl]
impl ScriptedToken {
    pub fn __constructor(env: Env) {
        env.storage().instance().set(&DataKey::FailArmed, &false);
    }

    /// When armed, any `transfer` whose `to` equals `fail_to` panics (full tx rollback).
    pub fn arm_fail_to(env: Env, fail_to: Address) {
        env.storage().instance().set(&DataKey::FailTo, &fail_to);
        env.storage().instance().set(&DataKey::FailArmed, &true);
    }

    pub fn disarm(env: Env) {
        env.storage().instance().set(&DataKey::FailArmed, &false);
    }

    pub fn mint(env: Env, to: Address, amount: i128) {
        let key = DataKey::Balance(to.clone());
        let bal: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&key, &(bal.checked_add(amount).expect("mint overflow")));
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Balance(id))
            .unwrap_or(0)
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        let armed: bool = env
            .storage()
            .instance()
            .get(&DataKey::FailArmed)
            .unwrap_or(false);
        if armed {
            let fail_to: Address = env.storage().instance().get(&DataKey::FailTo).unwrap();
            if to == fail_to {
                panic!("scripted token transfer refused");
            }
        }

        let from_key = DataKey::Balance(from.clone());
        let to_key = DataKey::Balance(to.clone());
        let from_bal: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);
        if from_bal < amount {
            panic!("insufficient balance");
        }
        let to_bal: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&from_key, &(from_bal - amount));
        env.storage()
            .persistent()
            .set(&to_key, &(to_bal.checked_add(amount).expect("overflow")));
    }

    pub fn decimals(_env: Env) -> u32 {
        7
    }
}
