use abstract_core::ibc_client::QueryMsgFns;
use abstract_core::objects::chain_name::ChainName;
use abstract_interface::{AbstractAccount, IbcClient};
use cw_orch::daemon::{ChainInfo, ChainKind, Daemon};
use cw_orch::prelude::ContractInstance;

use cw_orch::daemon::networks::juno::JUNO_NETWORK;
use std::io::{self, Write};

pub const JUNO_1: ChainInfo = ChainInfo {
    kind: ChainKind::Mainnet,
    chain_id: "juno-1",
    gas_denom: "ujuno",
    gas_price: 0.0950,
    grpc_urls: &["http://juno-grpc.polkachu.com:12690"],
    network_info: JUNO_NETWORK,
    lcd_url: None,
    fcd_url: None,
};

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
    println!(
        " Found {:?} remote proxies on: {:?}",
        remote_proxies,
        account.id()?,
    );
    Ok(remote_proxies)
}

pub fn press_enter_to_continue() {
    print!("Press enter to continue... ");
    io::stdout().flush().unwrap(); // Ensure the prompt is displayed immediately

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("Failed to read line");
}

pub const ABSTRACT_DEX_ADAPTER_ID: &str = "abstract:dex";
