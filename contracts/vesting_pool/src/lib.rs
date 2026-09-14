// contracts/vesting_pool/src/lib.rs
use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Symbol, Vec};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tranche {
    pub amount: i128,
    pub milestone_id: u32,
    pub unlocked: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VestingSchedule {
    pub beneficiary: Address,
    pub issuer: Address,
    pub tranches: Vec<Tranche>,
}

#[contracttype]
pub enum DataKey {
    Schedule(Address),
}

#[contract]
pub struct MilestoneVestingPoolContract;

#[contractimpl]
impl MilestoneVestingPoolContract {
    pub fn create_schedule(
        env: Env,
        issuer: Address,
        beneficiary: Address,
        tranches: Vec<Tranche>,
    ) {
        issuer.require_auth();

        let key = DataKey::Schedule(beneficiary.clone());
        if env.storage().persistent().has(&key) {
            panic!("Vesting schedule already exists for beneficiary");
        }

        let schedule = VestingSchedule {
            beneficiary,
            issuer,
            tranches,
        };
        env.storage().persistent().set(&key, &schedule);

        env.events()
            .publish((Symbol::new(&env, "ScheduleCreated"), beneficiary), issuer);
    }

    pub fn unlock_milestone(env: Env, admin: Address, beneficiary: Address, milestone_id: u32) {
        admin.require_auth();

        let key = DataKey::Schedule(beneficiary.clone());
        let mut schedule: VestingSchedule = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic!("Schedule not found"));

        if schedule.issuer != admin {
            panic!("Unauthorized: only the schedule issuer can unlock milestones");
        }

        let mut updated_tranches = Vec::new(&env);
        let mut found = false;
        for t in schedule.tranches.iter() {
            let mut tranche = t;
            if tranche.milestone_id == milestone_id {
                tranche.unlocked = true;
                found = true;
            }
            updated_tranches.push_back(tranche);
        }

        if !found {
            panic!("Milestone ID not found in schedule tranches");
        }

        schedule.tranches = updated_tranches;
        env.storage().persistent().set(&key, &schedule);

        env.events().publish(
            (Symbol::new(&env, "MilestoneUnlocked"), beneficiary),
            milestone_id,
        );
    }
}
