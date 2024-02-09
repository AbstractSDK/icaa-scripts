use abstract_core::ibc_client::QueryMsgFns;
use abstract_core::objects::chain_name::ChainName;
use abstract_interface::{AbstractAccount, IbcClient};
use cw_orch::daemon::Daemon;
use cw_orch::prelude::ContractInstance;

pub const IBC_CLIENT_ID: &str = "abstract:ibc-client";

// @feedback: it would be really nice to be able to query a module directly from the account
pub fn list_remote_proxies(
    chain: &Daemon,
    account: &AbstractAccount<Daemon>,
) -> anyhow::Result<Vec<(ChainName, Option<String>)>> {
    let ibc_client = IbcClient::new(IBC_CLIENT_ID, chain.clone());
    ibc_client.set_address(&account.manager.module_info(IBC_CLIENT_ID)?.unwrap().address);
    let remote_proxies = ibc_client
        .list_remote_proxies_by_account_id(account.id()?)?
        .proxies;
    println!(" {:?} remote_proxies: {:?}", account.id()?, remote_proxies);
    Ok(remote_proxies)
}
