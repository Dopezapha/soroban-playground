// contracts/cartel/src/lib.rs
use soroban_sdk::{contract, contractimpl, contracttype, Address, BytesN, Env, Symbol};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyndicateMember {
    pub member: Address,
    pub voting_power: i128,
    pub staked_amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub id: u64,
    pub proposer: Address,
    pub target_contract: Address,
    pub calldata_hash: BytesN<32>,
    pub yes_votes: i128,
    pub no_votes: i128,
    pub executed: bool,
    pub deadline: u64,
}

#[contracttype]
pub enum DataKey {
    Member(Address),
    Proposal(u64),
    ProposalCount,
    TotalStaked,
}

#[contract]
pub struct CartelSyndicateContract;

#[contractimpl]
impl CartelSyndicateContract {
    pub fn join_syndicate(env: Env, member: Address, amount: i128) {
        member.require_auth();
        if amount <= 0 {
            panic!("Stake amount must be positive");
        }

        let key = DataKey::Member(member.clone());
        let mut syndicate_member: SyndicateMember =
            env.storage()
                .persistent()
                .get(&key)
                .unwrap_or(SyndicateMember {
                    member: member.clone(),
                    voting_power: 0,
                    staked_amount: 0,
                });

        syndicate_member.staked_amount += amount;
        syndicate_member.voting_power += amount; // 1:1 token-weighted voting power

        let total_staked: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalStaked)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::TotalStaked, &(total_staked + amount));
        env.storage().persistent().set(&key, &syndicate_member);

        env.events()
            .publish((Symbol::new(&env, "MemberJoined"), member), amount);
    }

    pub fn create_proposal(
        env: Env,
        proposer: Address,
        target_contract: Address,
        calldata_hash: BytesN<32>,
        duration: u64,
    ) -> u64 {
        proposer.require_auth();

        let member_key = DataKey::Member(proposer.clone());
        let member: SyndicateMember = env
            .storage()
            .persistent()
            .get(&member_key)
            .unwrap_or_else(|| panic!("Caller is not a syndicate member"));

        if member.staked_amount <= 0 {
            panic!("Inactive members cannot create proposals");
        }

        let proposal_count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ProposalCount)
            .unwrap_or(0);
        let proposal_id = proposal_count + 1;
        let deadline = env.ledger().timestamp() + duration;

        let proposal = Proposal {
            id: proposal_id,
            proposer: proposer.clone(),
            target_contract,
            calldata_hash,
            yes_votes: 0,
            no_votes: 0,
            executed: false,
            deadline,
        };

        env.storage()
            .instance()
            .set(&DataKey::ProposalCount, &proposal_id);
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);

        env.events().publish(
            (Symbol::new(&env, "ProposalCreated"), proposal_id),
            proposer,
        );

        proposal_id
    }

    pub fn vote(env: Env, voter: Address, proposal_id: u64, support: bool) {
        voter.require_auth();

        let member_key = DataKey::Member(voter.clone());
        let member: SyndicateMember = env
            .storage()
            .persistent()
            .get(&member_key)
            .unwrap_or_else(|| panic!("Caller is not a syndicate member"));

        let prop_key = DataKey::Proposal(proposal_id);
        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&prop_key)
            .unwrap_or_else(|| panic!("Proposal not found"));

        if env.ledger().timestamp() > proposal.deadline {
            panic!("Voting period has expired");
        }

        if proposal.executed {
            panic!("Proposal already executed");
        }

        if support {
            proposal.yes_votes += member.voting_power;
        } else {
            proposal.no_votes += member.voting_power;
        }

        env.storage().persistent().set(&prop_key, &proposal);

        env.events().publish(
            (Symbol::new(&env, "Voted"), proposal_id),
            (voter, support, member.voting_power),
        );
    }
}
