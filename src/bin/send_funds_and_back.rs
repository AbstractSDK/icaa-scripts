use std::str::FromStr;

use abstract_client::{AbstractClient, Namespace};

use abstract_core::ibc_host::{HelperAction, HostAction};
use abstract_core::objects::chain_name::ChainName;
use abstract_core::PROXY;
use abstract_interface::{Abstract, AbstractAccount, ManagerExecFns};
use cosmwasm_std::coins;
use cw_orch::daemon::networks::juno::JUNO_NETWORK;
use cw_orch::daemon::networks::parse_network;
use cw_orch::daemon::queriers::Bank;
use cw_orch::daemon::{ChainInfo, ChainKind};
use cw_orch::{contract::Deploy, prelude::*};
use cw_orch_interchain::prelude::{ChannelCreationValidator, DaemonInterchainEnv, InterchainEnv};
use icaa_scripts::IBC_CLIENT_ID;
use pretty_env_logger::env_logger;
use tokio::runtime::Runtime;

use log::warn;

pub const JUNO_1: ChainInfo = ChainInfo {
    kind: ChainKind::Mainnet,
    chain_id: "juno-1",
    gas_denom: "ujuno",
    gas_price: 0.0750,
    grpc_urls: &["http://juno-grpc.polkachu.com:12690"],
    network_info: JUNO_NETWORK,
    lcd_url: None,
    fcd_url: None,
};

const HOME_CHAIN_ID: &str = "juno-1";
const HOME_CHAIN_NAME: &str = "juno";
const REMOTE_CHAIN_ID: &str = "archway-1";
const REMOTE_CHAIN_NAME: &str = "archway";

fn deploy() -> anyhow::Result<()> {
    let rt = Runtime::new()?;

    // Setup interchain environment
    let home_chain_info = parse_network(HOME_CHAIN_ID).unwrap();
    let remote_chain_info = parse_network(REMOTE_CHAIN_ID).unwrap();
    let interchain = DaemonInterchainEnv::new(
        rt.handle(),
        vec![(home_chain_info.clone(), None), (remote_chain_info, None)],
        &ChannelCreationValidator,
    )?;

    // Home chain is where all transactions originate
    let home = interchain.chain(HOME_CHAIN_ID)?;
    let home_denom = home_chain_info.gas_denom;

    // Sanity checks
    let sender = home.sender();
    let bank = home.query_client::<Bank>();
    let balance = rt
        .block_on(bank.balance(sender.clone(), Some(home_denom.to_string())))
        .unwrap();
    warn!("sender balance: {:?}", balance);

    // Setup
    let abstr = Abstract::load_from(home.clone())?;
    let client = AbstractClient::new(home.clone()).unwrap();
    let home_account_client = client
        .account_builder()
        .name("ICAA Test 2")
        .namespace(Namespace::new("icaa-test-2")?)
        .build()?;
    let home_acc = AbstractAccount::new(&abstr, home_account_client.id()?);

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

    let home_balance = home_account_client.query_balance(home_denom)?;
    warn!("Home balance after sending: {:?}", home_balance);

    // send funds back
    warn!("Requesting all funds back");
    let send_funds_tx = home_acc.manager.execute_on_module(
        "abstract:proxy",
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
