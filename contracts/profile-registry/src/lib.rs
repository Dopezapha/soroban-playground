#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, String};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserProfile {
    pub name: String,
    pub bio: String,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ProfileError {
    NameEmpty = 1,
    NameTooLong = 2,
    BioTooLong = 3,
    ProfileNotFound = 4,
}

#[contract]
pub struct ProfileRegistry;

const MAX_NAME_LEN: u32 = 64;
const MAX_BIO_LEN: u32 = 512;

#[contractimpl]
impl ProfileRegistry {
    /// Sets or updates a user profile.
    ///
    /// # Arguments
    /// * `user` - The Stellar address of the profile owner (must authorize)
    /// * `name` - Display name (1-64 characters)
    /// * `bio` - Biographical text (0-512 characters)
    pub fn set_profile(
        env: Env,
        user: Address,
        name: String,
        bio: String,
    ) -> Result<(), ProfileError> {
        user.require_auth();

        if name.is_empty() {
            return Err(ProfileError::NameEmpty);
        }
        if name.len() > MAX_NAME_LEN {
            return Err(ProfileError::NameTooLong);
        }
        if bio.len() > MAX_BIO_LEN {
            return Err(ProfileError::BioTooLong);
        }

        env.storage()
            .persistent()
            .set(&user, &UserProfile { name, bio });
        Ok(())
    }

    /// Retrieves a user profile by address.
    ///
    /// Returns `None` if no profile exists for the given address.
    pub fn get_profile(env: Env, user: Address) -> Option<UserProfile> {
        env.storage().persistent().get(&user)
    }

    /// Deletes a user profile.
    ///
    /// Returns an error if no profile exists for the given address.
    pub fn delete_profile(env: Env, user: Address) -> Result<(), ProfileError> {
        user.require_auth();

        if !env.storage().persistent().has(&user) {
            return Err(ProfileError::ProfileNotFound);
        }

        env.storage().persistent().remove(&user);
        Ok(())
    }

    /// Checks whether a profile exists for the given address.
    pub fn has_profile(env: Env, user: Address) -> bool {
        env.storage().persistent().has(&user)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn test_set_and_get_profile() {
        let env = Env::default();
        let contract_id = env.register_contract(None, ProfileRegistry);
        let client = ProfileRegistryClient::new(&env, &contract_id);

        let user = Address::generate(&env);
        let name = String::from_str(&env, "Alice");
        let bio = String::from_str(&env, "Blockchain Developer");

        env.mock_all_auths();

        client.set_profile(&user, &name, &bio).unwrap();

        let profile = client.get_profile(&user).unwrap();
        assert_eq!(profile.name, name);
        assert_eq!(profile.bio, bio);
    }

    #[test]
    fn test_get_profile_returns_none_when_missing() {
        let env = Env::default();
        let contract_id = env.register_contract(None, ProfileRegistry);
        let client = ProfileRegistryClient::new(&env, &contract_id);

        let user = Address::generate(&env);
        assert!(client.get_profile(&user).is_none());
    }

    #[test]
    fn test_overwrite_existing_profile() {
        let env = Env::default();
        let contract_id = env.register_contract(None, ProfileRegistry);
        let client = ProfileRegistryClient::new(&env, &contract_id);

        let user = Address::generate(&env);
        let name1 = String::from_str(&env, "Alice");
        let bio1 = String::from_str(&env, "Developer");
        let name2 = String::from_str(&env, "Alice Updated");
        let bio2 = String::from_str(&env, "Senior Developer");

        env.mock_all_auths();

        client.set_profile(&user, &name1, &bio1).unwrap();
        client.set_profile(&user, &name2, &bio2).unwrap();

        let profile = client.get_profile(&user).unwrap();
        assert_eq!(profile.name, name2);
        assert_eq!(profile.bio, bio2);
    }

    #[test]
    fn test_set_profile_rejects_empty_name() {
        let env = Env::default();
        let contract_id = env.register_contract(None, ProfileRegistry);
        let client = ProfileRegistryClient::new(&env, &contract_id);

        let user = Address::generate(&env);
        let name = String::from_str(&env, "");
        let bio = String::from_str(&env, "Developer");

        env.mock_all_auths();

        let result = client.try_set_profile(&user, &name, &bio);
        assert_eq!(result, Err(Ok(ProfileError::NameEmpty)));
    }

    #[test]
    fn test_delete_profile() {
        let env = Env::default();
        let contract_id = env.register_contract(None, ProfileRegistry);
        let client = ProfileRegistryClient::new(&env, &contract_id);

        let user = Address::generate(&env);
        let name = String::from_str(&env, "Alice");
        let bio = String::from_str(&env, "Developer");

        env.mock_all_auths();

        client.set_profile(&user, &name, &bio).unwrap();
        assert!(client.has_profile(&user));

        client.delete_profile(&user).unwrap();
        assert!(!client.has_profile(&user));
        assert!(client.get_profile(&user).is_none());
    }

    #[test]
    fn test_delete_nonexistent_profile_returns_error() {
        let env = Env::default();
        let contract_id = env.register_contract(None, ProfileRegistry);
        let client = ProfileRegistryClient::new(&env, &contract_id);

        let user = Address::generate(&env);

        env.mock_all_auths();

        let result = client.try_delete_profile(&user);
        assert_eq!(result, Err(Ok(ProfileError::ProfileNotFound)));
    }

    #[test]
    fn test_has_profile() {
        let env = Env::default();
        let contract_id = env.register_contract(None, ProfileRegistry);
        let client = ProfileRegistryClient::new(&env, &contract_id);

        let user = Address::generate(&env);

        env.mock_all_auths();

        assert!(!client.has_profile(&user));

        let name = String::from_str(&env, "Alice");
        let bio = String::from_str(&env, "Dev");
        client.set_profile(&user, &name, &bio).unwrap();

        assert!(client.has_profile(&user));
    }
}
