use std::str::FromStr;

use abstract_client::{AbstractClient, Namespace};
use abstract_core::ibc_client::QueryMsgFns;
use abstract_core::ibc_host::{HelperAction, HostAction};
use abstract_core::objects::AssetEntry;
use abstract_core::objects::chain_name::ChainName;
use abstract_interface::{Abstract, AbstractAccount, IbcClient, ManagerExecFns};
use cosmwasm_std::coins;
use cw_orch::{
    contract::Deploy,
    prelude::*,
};
use cw_orch::daemon::{ChainInfo, ChainKind};
use cw_orch::daemon::networks::{ARCHWAY_1, OSMOSIS_1};
use cw_orch::daemon::networks::juno::JUNO_NETWORK;
use cw_orch::daemon::queriers::Bank;
use cw_orch_interchain::prelude::{ChannelCreationValidator, DaemonInterchainEnv, InterchainEnv};
use pretty_env_logger::env_logger;
use tokio::runtime::Runtime;

pub const JUNO_1: ChainInfo = ChainInfo {
    kind: ChainKind::Mainnet,
    chain_id: "juno-1",
    gas_denom: "ujuno",
    gas_price: 0.0750,
    grpc_urls: &[
        "http://juno-grpc.polkachu.com:12690",
    ],
    network_info: JUNO_NETWORK,
    lcd_url: None,
    fcd_url: None,
};

const IBC_CLIENT_ID: &'static str = "abstract:ibc-client";

const REMOTE_CHAIN_NAME: &str = "archway";

fn deploy() -> anyhow::Result<()> {
    let rt = Runtime::new()?;


    let interchain = DaemonInterchainEnv::new(rt.handle(), vec![(JUNO_1, None), (ARCHWAY_1, None)], &ChannelCreationValidator)?;
    // let interchain = DaemonInterchainEnv::new(rt.handle(), vec![(JUNO_1, None), (OSMOSIS_1, None)], &ChannelCreationValidator)?;

    // setup juno chain
    let juno = interchain.chain(JUNO_1.chain_id)?;

    let archway = interchain.chain(ARCHWAY_1.chain_id)?;
    // let osmosis = interchain.chain(OSMOSIS_1.chain_id)?;

    // Sanity checks
    let sender = juno.sender();
    let bank = juno.query_client::<Bank>();
    let balance = rt.block_on(bank.balance(sender.clone(), Some("ujuno".to_string()))).unwrap();
    println!("balance: {:?}", balance);

    // Setup
    let abstr = Abstract::load_from(juno.clone())?;
    let client = AbstractClient::new(juno.clone()).unwrap();
    let home_account_client = client.account_builder().name("ICAA Test 2").namespace(Namespace::new("icaa-test-2")?).build()?;
    let home_account_id = home_account_client.id()?;
    let home_acc = AbstractAccount::new(&abstr, home_account_id.clone());

    // Check and enable IBC
    if !home_acc.manager.is_module_installed(IBC_CLIENT_ID)? {
        println!("Enabling IBC");
        home_acc.manager.update_settings(Some(true))?;
    }

    // CHeck for and register remote account on osmosis
    // @feedback: it would be really nice to be able to query a module directly from the account
    let mut remote_proxies= list_remote_proxies(&juno, &home_acc)?;

    // could sanity check that osmosis is an available host
    // let remote_hosts = ibc_client.list_remote_hosts()?.hosts;
    // println!("hosts: {:?}", remote_hosts);

    let home_chain_id = JUNO_1.chain_id.to_string();

    if remote_proxies.iter().find(|(chain, _)| chain == &ChainName::from_str(REMOTE_CHAIN_NAME).unwrap()).is_none() {
        println!("Registering remote account on {}", REMOTE_CHAIN_NAME);
        let remote_acc_tx = home_acc.register_remote_account(REMOTE_CHAIN_NAME)?;
        // @feedback chain id or chain name?
        interchain.wait_ibc(&home_chain_id, remote_acc_tx)?;

        remote_proxies = list_remote_proxies(&juno, &home_acc)?;
        println!("remote_proxies: {:?}", remote_proxies);
    }

    // Check home account balance before sending
    let home_balance = home_account_client.query_balance("ujuno")?;
    if home_balance.is_zero() {
        // @feedback make it easier to send funds from wallet?
        //  - maybe a acc_client.deposit() method
        rt.block_on(juno.daemon.sender.bank_send(home_account_client.proxy()?.as_str(), coins(500, "ujuno")))?;
    }

    // Send funds to the remote account
    // @feedback would be great to have a send_funds method on the manager that would accept resolvable
    //  - and an execute_ibc or close
    // let osmo_info = abstr.ans_host.resolve(&AssetEntry::from("osmosis>osmo"))?;
    let send_funds_tx = home_acc.manager.execute_on_module(
        "abstract:proxy",
        abstract_core::proxy::ExecuteMsg::IbcAction {
            msgs: vec![abstract_core::ibc_client::ExecuteMsg::SendFunds {
                host_chain: REMOTE_CHAIN_NAME.into(),
                funds: coins(home_balance.u128(), "ujuno"),
            }],
        },
    )?;
    interchain.wait_ibc(&home_chain_id, send_funds_tx)?;

    let home_balance = home_account_client.query_balance("ujuno")?;
    println!("Home balance after sending: {:?}", home_balance);

    // send funds back
    println!("Requesting all funds back");
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

    interchain.wait_ibc(&home_chain_id, send_funds_tx)?;

    let home_balance = home_account_client.query_balance("ujuno")?;
    println!("Home balance after receiving back: {:?}", home_balance);


    // Currently send funds, send back
    // maybe Send juno, swap juno for osmo, send back?


    Ok(())
}

fn list_remote_proxies(chain: &Daemon, account: &AbstractAccount<Daemon>) -> anyhow::Result<Vec<(ChainName, Option<String>)>> {
    let mut ibc_client = IbcClient::new(IBC_CLIENT_ID, chain.clone());
    ibc_client.set_address(&account.manager.module_info(IBC_CLIENT_ID)?.unwrap().address);
    let mut remote_proxies = ibc_client.list_remote_proxies_by_account_id(account.id()?)?.proxies;
    println!("remote_proxies: {:?}", remote_proxies);
    Ok(remote_proxies)
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
