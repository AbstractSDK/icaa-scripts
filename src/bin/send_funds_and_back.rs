use std::str::FromStr;

use abstract_client::{AbstractClient, Namespace};

use abstract_core::ibc_host::{HelperAction, HostAction};

use abstract_core::objects::chain_name::ChainName;
use abstract_core::objects::gov_type::GovernanceDetails;
use abstract_core::objects::{AccountId, AssetEntry};

use abstract_core::PROXY;
use abstract_interface::{Abstract, AbstractAccount, ManagerExecFns};
use cosmwasm_std::{coins, Uint128};
use cw_asset::AssetInfo;
use cw_orch::daemon::networks::parse_network;
use cw_orch::daemon::queriers::Bank;
use cw_orch::environment::BankQuerier;
use cw_orch::{contract::Deploy, prelude::*};
use cw_orch_interchain::prelude::{ChannelCreationValidator, DaemonInterchainEnv, InterchainEnv};
use icaa_scripts::{press_enter_to_continue, IBC_CLIENT_ID, JUNO_1};
use pretty_env_logger::env_logger;
use tokio::runtime::Runtime;

use log::warn;

const HOME_CHAIN_ID: &str = "juno-1";
const HOME_CHAIN_NAME: &str = "juno";
const REMOTE_CHAIN_ID: &str = "archway-1";
const REMOTE_CHAIN_NAME: &str = "archway";

fn deploy() -> anyhow::Result<()> {
    let rt = Runtime::new()?;

    // Setup interchain environment
    let home_chain_info = JUNO_1; // let home_chain_info = parse_network(HOME_CHAIN_ID).unwrap();
    let remote_chain_info = parse_network(REMOTE_CHAIN_ID).unwrap();
    let interchain = DaemonInterchainEnv::new(
        rt.handle(),
        vec![(home_chain_info.clone(), None), (remote_chain_info, None)],
        &ChannelCreationValidator,
    )?;

    // Home chain is where all transactions originate
    let home = interchain.chain(HOME_CHAIN_ID)?;
    let home_denom = home_chain_info.gas_denom;
    let remote = interchain.chain(REMOTE_CHAIN_ID)?;

    // Sanity checks
    let sender = home.sender();
    let bank = home.query_client::<Bank>();
    let balance = rt
        .block_on(bank.balance(sender.clone(), Some(home_denom.to_string())))
        .unwrap();
    warn!("sender balance: {:?}", balance);

    // Setup abstract + home account
    let home_abstr = Abstract::load_from(home.clone())?;
    let home_client = AbstractClient::new(home.clone()).unwrap();
    let home_account_client = home_client
        .account_builder()
        .name("ICAA Test 2")
        .namespace(Namespace::new("icaa-test-2")?)
        .build()?;
    let home_acc = AbstractAccount::new(&home_abstr, home_account_client.id()?);

    // Sub-account (where we're performing our test)
    let home_account_client = home_client
        .account_builder()
        .name("ICAA Test Juno Osmosis")
        .namespace(Namespace::new("icaa-test-juno-osmosis")?)
        .ownership(GovernanceDetails::SubAccount {
            proxy: home_acc.proxy.addr_str()?,
            manager: home_acc.manager.addr_str()?,
        })
        .build()?;
    let home_acc = AbstractAccount::new(&home_abstr, home_account_client.id()?);

    press_enter_to_continue();

    // Check and enable IBC on home chain
    if !home_acc.manager.is_module_installed(IBC_CLIENT_ID)? {
        warn!("Enabling IBC on {}", HOME_CHAIN_NAME);
        home_acc.manager.update_settings(Some(true))?;
    } else {
        warn!("IBC is already enabled on {}!", HOME_CHAIN_NAME);
    }

    // could sanity check that osmosis is an available host
    // let remote_hosts = ibc_client.list_remote_hosts()?.hosts;
    // warn!("hosts: {:?}", remote_hosts);

    // CHeck for and register remote account on osmosis
    let mut remote_proxies = icaa_scripts::list_remote_proxies(&home, &home_acc)?;

    if !remote_proxies
        .iter()
        .any(|(chain, _)| chain == &ChainName::from_str(REMOTE_CHAIN_NAME).unwrap())
    {
        warn!("Registering remote account on {}", REMOTE_CHAIN_NAME);
        let remote_acc_tx = home_acc.register_remote_account(REMOTE_CHAIN_NAME)?;
        // @feedback chain id or chain name?
        interchain.wait_ibc(&HOME_CHAIN_ID.into(), remote_acc_tx)?;

        remote_proxies = icaa_scripts::list_remote_proxies(&home, &home_acc)?;
        warn!("remote_proxies: {:?}", remote_proxies);
    } else {
        warn!("{} already registered", REMOTE_CHAIN_NAME);
    }

    press_enter_to_continue();
    let remote_balance = get_remote_balance(&remote, &home_acc)?;
    warn!("Remote balance before sending: {:?}", remote_balance);
    press_enter_to_continue();

    // Check home account balance before sending
    let home_balance = home_account_client.query_balance(home_denom)?;
    warn!("Home balance is: {}", home_balance);
    if home_balance.is_zero() {
        warn!("Sending some funds from wallet to account.");
        // @feedback make it easier to send funds from wallet?
        //  - maybe a acc_client.deposit() method
        rt.block_on(home.daemon.sender.bank_send(
            home_account_client.proxy()?.as_str(),
            coins(500, home_denom),
        ))?;
    }

    // Send funds to the remote account
    warn!(
        "Sending funds from {} to {}.",
        HOME_CHAIN_ID, REMOTE_CHAIN_NAME
    );
    let send_funds_tx = home_acc.manager.execute_on_module(
        PROXY,
        abstract_core::proxy::ExecuteMsg::IbcAction {
            msgs: vec![abstract_core::ibc_client::ExecuteMsg::SendFunds {
                host_chain: REMOTE_CHAIN_NAME.into(),
                funds: coins(home_balance.u128(), home_denom),
            }],
        },
    )?;
    interchain.wait_ibc(&HOME_CHAIN_ID.into(), send_funds_tx)?;

    // Check both balances
    let home_balance = home_account_client.query_balance(home_denom)?;
    warn!("Home balance after sending: {:?}", home_balance);

    let remote_balance = get_remote_balance(&remote, &home_acc)?;
    warn!("Remote balance after sending: {:?}", remote_balance);

    press_enter_to_continue();

    // send funds back
    warn!("Requesting all funds back");
    let send_funds_tx = home_acc.manager.execute_on_module(
        PROXY,
        abstract_core::proxy::ExecuteMsg::IbcAction {
            msgs: vec![abstract_core::ibc_client::ExecuteMsg::RemoteAction {
                host_chain: REMOTE_CHAIN_NAME.into(),
                action: HostAction::Helpers(HelperAction::SendAllBack),
                callback_info: None,
            }],
        },
    )?;

    interchain.wait_ibc(&HOME_CHAIN_ID.into(), send_funds_tx)?;

    let home_balance = home_account_client.query_balance(home_denom)?;
    warn!("Home balance after receiving back: {:?}", home_balance);

    // Currently send funds, send back
    // maybe Send juno, swap juno for osmo, send back?

    Ok(())
}

fn get_remote_balance(
    remote: &Daemon,
    home_acc: &AbstractAccount<Daemon>,
) -> anyhow::Result<Uint128> {
    // Get the archway remote hosts
    let remote_account_id = AccountId::remote(
        home_acc.id()?.seq(),
        vec![ChainName::from_str(HOME_CHAIN_NAME)?],
    )?;
    let remote_acc = AbstractAccount::new(
        &Abstract::load_from(remote.clone())?,
        remote_account_id.clone(),
    );
    let remote_balances = remote.balance(remote_acc.proxy.address()?, None)?;
    println!("Remote balances: {:?}", remote_balances);

    let remote_abstr = Abstract::load_from(remote.clone())?;
    let juno_info = remote_abstr
        .ans_host
        .resolve(&AssetEntry::from("juno>juno"))?;
    let juno_denom = match juno_info {
        AssetInfo::Native(address) => address,
        _ => anyhow::bail!("juno is not a token"),
    };

    let remote_balance = remote.balance(remote_acc.proxy.address()?, Some(juno_denom))?[0].amount;

    Ok(remote_balance)
}

fn main() {
    dotenv().ok();
    env_logger::init();

    use dotenv::dotenv;

    if let Err(ref err) = deploy() {
        log::error!("{}", err);
        err.chain()
            .skip(1)
            .for_each(|cause| log::error!("because: {}", cause));

        // The backtrace is not always generated. Try to run this example

        // with `$env:RUST_BACKTRACE=1`.

        //    if let Some(backtrace) = err.un.backtrace() {
        //        log::debug!("backtrace: {:?}", backtrace);
        //    }

        ::std::process::exit(1);
    }
}

/*
       // Regiser base asset on remote
       let register_base_asset_tx = home_acc.manager.execute_on_remote_module(REMOTE_CHAIN_NAME, PROXY, to_json_binary(&abstract_core::proxy::ExecuteMsg::UpdateAssets {
           to_add: vec![(AssetEntry::from("juno>juno"), UncheckedPriceSource::None)],
           to_remove: vec![],
       })?, None)?;
       interchain.wait_ibc(&HOME_CHAIN_ID.into(), register_base_asset_tx)?;

*/
