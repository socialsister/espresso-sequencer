use alloy::{
    primitives::{utils::format_ether, Address},
    providers::ProviderBuilder,
};
use anyhow::{Context as _, Result};
use espresso_types::{
    v0_3::{Fetcher, Validator},
    L1Client,
};
use hotshot_contract_adapter::sol_types::StakeTableV2;
use hotshot_types::signature_key::BLSPubKey;
use url::Url;

use crate::parse::Commission;

pub async fn stake_table_info(
    l1_url: Url,
    stake_table_address: Address,
    l1_block_number: u64,
) -> Result<Vec<Validator<BLSPubKey>>> {
    let l1 = L1Client::new(vec![l1_url])?;
    let validators =
        Fetcher::fetch_all_validators_from_contract(l1, stake_table_address, l1_block_number)
            .await?;

    Ok(validators
        .into_iter()
        .map(|(_address, validator)| validator)
        .collect())
}

pub fn display_stake_table(stake_table: Vec<Validator<BLSPubKey>>, compact: bool) -> Result<()> {
    let mut stake_table = stake_table.clone();
    stake_table.sort_by(|a, b| a.stake.cmp(&b.stake));

    for validator in stake_table.iter() {
        let comm: Commission = validator.commission.try_into()?;
        let bls_key = validator.stake_table_key.to_string();
        let key_str = if compact {
            let end = bls_key.chars().map(|c| c.len_utf8()).take(40).sum();
            format!("{}..", &bls_key[..end])
        } else {
            bls_key.to_string()
        };
        tracing::info!(
            "Validator {}: {key_str} comm={comm} stake={} ESP",
            validator.account,
            format_ether(validator.stake),
        );

        if validator.delegators.is_empty() {
            tracing::info!(" - No delegators");
            continue;
        }

        // sort delegators by address for easier reading
        let mut delegators = validator.delegators.iter().collect::<Vec<_>>();
        delegators.sort_by(|a, b| a.0.cmp(b.0));
        for (delegator, stake) in delegators {
            tracing::info!(
                " - Delegator {delegator}: stake={} ESP",
                format_ether(*stake)
            );
        }
    }
    Ok(())
}

pub async fn fetch_token_address(rpc_url: Url, stake_table_address: Address) -> Result<Address> {
    let provider = ProviderBuilder::new().on_http(rpc_url);
    Ok(StakeTableV2::new(stake_table_address, provider)
        .token()
        .call()
        .await
        .with_context(|| {
            format!(
                "Failed to fetch token address from stake table contract at {stake_table_address}"
            )
        })?
        ._0)
}
