use std::sync::Arc;

use anchor_lang::{
    prelude::{AccountMeta, Pubkey},
    AccountSerialize, AnchorSerialize, Discriminator,
};
use borsh::BorshSerialize;
use governance_rewards::state::{
    addin::VoterWeightRecord, claim_data::ClaimData, distribution::Distribution,
    distribution_option::DistributionOptions, preferences::UserPreferences,
};
use solana_program::instruction::Instruction;
use solana_program_test::{processor, ProgramTest};
use solana_sdk::{
    account::AccountSharedData, signature::Keypair, signer::Signer, transport::TransportError,
};

use super::{
    governance_test::{GovernanceTest, RealmCookie},
    program_test_bench::{MintCookie, ProgramTestBench, TokenAccountCookie},
    tools::NopOverride,
};

#[derive(Debug)]
pub struct DistributionCookie {
    pub address: Pubkey,
    pub admin: Keypair,
    pub account: Distribution,

    pub registration_cutoff: u64,
}

#[derive(Debug)]
pub struct DistributionKeyCookie {
    pub keypair: Keypair,
}

#[derive(Debug)]
pub struct RegistrantCookie {
    user: Pubkey,
}

#[derive(Debug)]
pub struct VoterWeightRecordCookie {
    pub address: Pubkey,
    pub user: Pubkey,
    pub weight: u64,
}

pub struct GovernanceRewardsTest {
    pub program_id: Pubkey,
    pub bench: Arc<ProgramTestBench>,
    pub governance: GovernanceTest,
}

fn voter_weight_program() -> Pubkey {
    Pubkey::try_from("4Q6WW2ouZ6V3iaNm56MTd5n2tnTm4C5fiH8miFHnAFHo").unwrap()
}

impl GovernanceRewardsTest {
    pub fn add_program(program_test: &mut ProgramTest) {
        program_test.add_program(
            "governance_rewards",
            governance_rewards::id(),
            processor!(governance_rewards::entry),
        );
    }

    pub async fn start_new() -> Self {
        let mut program_test = ProgramTest::default();

        GovernanceRewardsTest::add_program(&mut program_test);
        GovernanceTest::add_program(&mut program_test);

        let program_id = governance_rewards::id();

        let bench = ProgramTestBench::start_new(program_test).await;
        let bench_rc = Arc::new(bench);

        let governance_bench =
            GovernanceTest::new(bench_rc.clone(), Some(program_id), Some(program_id));

        Self {
            program_id,
            bench: bench_rc,
            governance: governance_bench,
        }
    }

    pub fn with_distribution_keypair(&self) -> DistributionKeyCookie {
        DistributionKeyCookie {
            keypair: Keypair::new(),
        }
    }

    pub async fn with_funded_distribution(
        &mut self,
        realm_cookie: &RealmCookie,
        key: &DistributionKeyCookie,
        registration_cutoff: u64,
    ) -> Result<DistributionCookie, TransportError> {
        let funding_amount = 100;
        let funding_mint = self.bench.with_mint().await?;
        let funding_account = self
            .with_owned_tokens(&funding_mint, key, funding_amount)
            .await?;

        self.with_distribution(realm_cookie, key, registration_cutoff, &[&funding_account])
            .await
    }

    pub async fn with_distribution(
        &mut self,
        realm_cookie: &RealmCookie,
        key: &DistributionKeyCookie,
        registration_cutoff: u64,
        funding: &[&TokenAccountCookie],
    ) -> Result<DistributionCookie, TransportError> {
        self.with_distribution_using_ix(
            realm_cookie,
            key,
            registration_cutoff,
            funding,
            NopOverride,
            None,
        )
        .await
    }

    pub async fn with_distribution_using_ix<F: Fn(&mut Instruction)>(
        &mut self,
        realm_cookie: &RealmCookie,
        key: &DistributionKeyCookie,
        registration_cutoff: u64,
        funding: &[&TokenAccountCookie],
        instruction_override: F,
        signers_override: Option<&[&Keypair]>,
    ) -> Result<DistributionCookie, TransportError> {
        let data = anchor_lang::InstructionData::data(
            &governance_rewards::instruction::CreateDistribution {
                registration_cutoff,
            },
        );
        let admin = Keypair::new();

        let mut accounts = anchor_lang::ToAccountMetas::to_account_metas(
            &governance_rewards::accounts::CreateDistribution {
                distribution: key.keypair.pubkey(),
                payout_authority: Distribution::get_payout_authority(key.keypair.pubkey()),
                realm: realm_cookie.address,
                voter_weight_program: voter_weight_program(),
                payer: self.bench.payer.pubkey(),
                admin: admin.pubkey(),
                system_program: solana_sdk::system_program::id(),
            },
            None,
        );

        for funding_source in funding {
            accounts.push(AccountMeta::new_readonly(funding_source.address, false));
        }

        let mut create_distribution_ix = Instruction {
            program_id: governance_rewards::id(),
            accounts,
            data,
        };

        instruction_override(&mut create_distribution_ix);

        let default_signers = &[&key.keypair, &admin];
        let signers = signers_override.unwrap_or(default_signers);

        self.bench
            .process_transaction(&[create_distribution_ix], Some(signers))
            .await?;

        let account = Distribution {
            registration_period_end_ts: registration_cutoff,
            voter_weight_program: voter_weight_program(),
            realm: realm_cookie.address,
            total_vote_weight: 0,
            total_vote_weight_claimed: 0,
            distribution_options: DistributionOptions::empty(),
            admin: admin.pubkey(),
        };

        Ok(DistributionCookie {
            address: key.keypair.pubkey(),
            account,
            admin,
            registration_cutoff,
        })
    }

    pub async fn get_distribution_account(&mut self, distribution: Pubkey) -> Distribution {
        self.bench
            .get_anchor_account::<Distribution>(distribution)
            .await
    }

    pub async fn with_owned_tokens(
        &self,
        mint: &MintCookie,
        distribution: &DistributionKeyCookie,
        amount: u64,
    ) -> Result<TokenAccountCookie, TransportError> {
        let owner = Distribution::get_payout_authority(distribution.keypair.pubkey());
        self.bench.with_tokens(mint, &owner, amount).await
    }

    pub async fn with_preferences(
        &mut self,
        record: &UserPreferences,
        realm: &RealmCookie,
        user: Pubkey,
    ) -> Result<(), TransportError> {
        dbg!(UserPreferences::discriminator());
        let address = UserPreferences::get_address(user, realm.address);
        self.set_anchor_account(record, address, self.program_id)
            .await
    }

    pub async fn with_dummy_voter_weight_record(
        &mut self,
        record: &VoterWeightRecord,
        owner: Pubkey,
    ) -> Result<VoterWeightRecordCookie, TransportError> {
        let key = Keypair::new().pubkey();
        self.set_borsht_account(record, key, owner).await?;

        Ok(VoterWeightRecordCookie {
            address: key,
            user: record.governing_token_owner,
            weight: record.voter_weight,
        })
    }

    pub async fn set_anchor_account<T: AccountSerialize>(
        &mut self,
        record: &T,
        address: Pubkey,
        owner: Pubkey,
    ) -> Result<(), TransportError> {
        let mut data: Vec<u8> = Vec::new();
        record.try_serialize(&mut data).unwrap();
        self.set_account(data, address, owner).await
    }

    pub async fn set_borsht_account<T: AnchorSerialize>(
        &mut self,
        record: &T,
        address: Pubkey,
        owner: Pubkey,
    ) -> Result<(), TransportError> {
        let data: Vec<u8> = record.try_to_vec().unwrap();
        self.set_account(data, address, owner).await
    }

    pub async fn set_account(
        &mut self,
        data: Vec<u8>,
        address: Pubkey,
        owner: Pubkey,
    ) -> Result<(), TransportError> {
        let lamports = {
            let rent = self
                .bench
                .context
                .borrow_mut()
                .banks_client
                .get_rent()
                .await?;
            rent.minimum_balance(data.len())
        };
        let mut account_data = AccountSharedData::new(lamports, data.len(), &owner);
        account_data.set_data(data);
        self.bench
            .context
            .borrow_mut()
            .set_account(&address, &account_data);
        Ok(())
    }

    pub async fn with_registrant(
        &mut self,
        distribution_cookie: &DistributionCookie,
        voter_weight_record_cookie: &VoterWeightRecordCookie,
    ) -> Result<RegistrantCookie, TransportError> {
        self.with_registrant_using_ix(
            distribution_cookie,
            voter_weight_record_cookie,
            NopOverride,
            None,
        )
        .await
    }

    pub async fn with_registrant_using_ix<F: Fn(&mut Instruction)>(
        &mut self,
        distribution_cookie: &DistributionCookie,
        voter_weight_record_cookie: &VoterWeightRecordCookie,
        instruction_override: F,
        signers_override: Option<&[&Keypair]>,
    ) -> Result<RegistrantCookie, TransportError> {
        let data =
            anchor_lang::InstructionData::data(&governance_rewards::instruction::Register {});
        let accounts = anchor_lang::ToAccountMetas::to_account_metas(
            &governance_rewards::accounts::RegisterForRewards {
                voter_weight_record: voter_weight_record_cookie.address,
                distribution: distribution_cookie.address,
                preferences: UserPreferences::get_address(
                    voter_weight_record_cookie.user,
                    distribution_cookie.account.realm,
                ),
                claim_data: ClaimData::get_address(
                    voter_weight_record_cookie.user,
                    distribution_cookie.address,
                ),
                registrant: voter_weight_record_cookie.user,
                payer: self.bench.payer.pubkey(),
                system_program: solana_sdk::system_program::id(),
            },
            None,
        );

        let mut register_ix = Instruction {
            program_id: governance_rewards::id(),
            accounts,
            data,
        };

        instruction_override(&mut register_ix);

        let default_signers = &[&self.bench.payer];
        let signers = signers_override.unwrap_or(default_signers);

        self.bench
            .process_transaction(&[register_ix], Some(signers))
            .await?;

        Ok(RegistrantCookie {
            user: voter_weight_record_cookie.user,
        })
    }
}
