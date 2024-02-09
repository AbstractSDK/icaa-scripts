use std::str::FromStr;

use abstract_client::{AbstractClient, Namespace};
use abstract_core::ibc_client::QueryMsgFns;
use abstract_core::ibc_host::{HelperAction, HostAction};
use abstract_core::manager::ModuleInstallConfig;
use abstract_core::objects::account::AccountTrace;
use abstract_core::objects::chain_name::ChainName;
use abstract_core::objects::module::ModuleInfo;
use abstract_core::objects::{AccountId, AssetEntry};
use abstract_core::{manager, PROXY};
use abstract_interface::{Abstract, AbstractAccount, IbcClient, ManagerExecFns};
use cosmwasm_std::{coins, to_json_binary};
use cw_orch::daemon::networks::juno::JUNO_NETWORK;
use cw_orch::daemon::networks::{ARCHWAY_1, OSMOSIS_1};
use cw_orch::daemon::queriers::Bank;
use cw_orch::daemon::{ChainInfo, ChainKind};
use cw_orch::{contract::Deploy, prelude::*};
use cw_orch_interchain::prelude::{ChannelCreationValidator, DaemonInterchainEnv, InterchainEnv};
use icaa_scripts::list_remote_proxies;
use pretty_env_logger::env_logger;
use tokio::runtime::Runtime;

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

const IBC_CLIENT_ID: &'static str = "abstract:ibc-client";

const HOME_CHAIN_NAME: &str = "juno";
const FIRST_HOP_CHAIN_NAME: &str = "archway";
const SECOND_HOP_CHAIN_NAME: &str = "juno";
const THIRD_HOP_CHAIN_NAME: &str = "archway";

fn deploy() -> anyhow::Result<()> {
    let rt = Runtime::new()?;

    let interchain = DaemonInterchainEnv::new(
        rt.handle(),
        vec![(JUNO_1, None), (ARCHWAY_1, None), (OSMOSIS_1, None)],
        &ChannelCreationValidator,
    )?;
    // interchain.with_log();

    // setup juno chain
    let juno = interchain.chain(JUNO_1.chain_id)?;
    let archway = interchain.chain(ARCHWAY_1.chain_id)?;
    let osmosis = interchain.chain(OSMOSIS_1.chain_id)?;

    // Sanity checks
    let sender = juno.sender();
    let bank = juno.query_client::<Bank>();
    let balance = rt
        .block_on(bank.balance(sender.clone(), Some("ujuno".to_string())))
        .unwrap();
    println!("balance: {:?}", balance);

    // Setup
    let juno_abstr = Abstract::load_from(juno.clone())?;
    let juno_client = AbstractClient::new(juno.clone()).unwrap();
    let home_account_client = juno_client
        .account_builder()
        .namespace(Namespace::new("icaa-cross-back")?)
        .build()?;
    let home_account_id = home_account_client.id()?;
    let home_acc = AbstractAccount::new(&juno_abstr, home_account_id.clone());

    // Check and enable IBC
    if !home_acc.manager.is_module_installed(IBC_CLIENT_ID)? {
        println!("Enabling IBC");
        home_acc.manager.update_settings(Some(true))?;
    }

    // CHeck for and register remote account on osmosis
    let mut remote_proxies = list_remote_proxies(&juno, &home_acc)?;
    println!("accounts: {:?}", remote_proxies);

    // could sanity check that osmosis is an available host
    // let remote_hosts = ibc_client.list_remote_hosts()?.hosts;
    // println!("hosts: {:?}", remote_hosts);

    let home_chain_id = JUNO_1.chain_id.to_string();

    // check for archway
    if remote_proxies
        .iter()
        .find(|(chain, _)| chain == &ChainName::from_str(FIRST_HOP_CHAIN_NAME).unwrap())
        .is_none()
    {
        println!("Registering remote account on archway");
        let remote_acc_tx = home_acc.register_remote_account(FIRST_HOP_CHAIN_NAME)?;
        // @feedback chain id or chain name?
        interchain.wait_ibc(&home_chain_id, remote_acc_tx)?;

        remote_proxies = list_remote_proxies(&juno, &home_acc)?;
        println!("accounts: {:?}", remote_proxies);
    } else {
        println!("Found remote proxy!")
    }

    // Get the archway remote hosts
    let archway_account_id = AccountId::new(
        home_acc.id()?.seq(),
        AccountTrace::Remote(vec![ChainName::from_str(HOME_CHAIN_NAME)?]),
    )?;
    let archway_acc = AbstractAccount::new(
        &Abstract::load_from(archway.clone())?,
        archway_account_id.clone(),
    );

    // Check and enable IBC on Archway
    if !archway_acc.manager.is_module_installed(IBC_CLIENT_ID)? {
        println!("Enabling IBC on Archway");
        let enable_ibc_tx = home_acc.manager.execute_on_remote(
            FIRST_HOP_CHAIN_NAME.into(),
            manager::ExecuteMsg::UpdateSettings {
                ibc_enabled: Some(true),
            },
            None,
        )?;
        interchain.wait_ibc(&home_chain_id, enable_ibc_tx)?;
    } else {
        println!("Ibc client is installed on Archway!");
    }

    let mut remote_proxies = list_remote_proxies(&archway, &archway_acc)?;
    println!("archway remote_proxies: {:?}", remote_proxies);

    // check whether juno>archway has registered osmosis
    if remote_proxies
        .iter()
        .find(|(chain, _)| chain == &ChainName::from_str(SECOND_HOP_CHAIN_NAME).unwrap())
        .is_none()
    {
        println!("Registering remote account from archway on osmosis");
        let home_manager = &home_acc.manager;
        let result = home_manager.execute_on_remote_module(
            FIRST_HOP_CHAIN_NAME.into(),
            PROXY,
            to_json_binary(&abstract_core::proxy::ExecuteMsg::IbcAction {
                msgs: vec![abstract_core::ibc_client::ExecuteMsg::Register {
                    host_chain: SECOND_HOP_CHAIN_NAME.into(),
                    base_asset: None,
                    namespace: None,
                    install_modules: vec![],
                    // install_modules: vec![ModuleInstallConfig::new(ModuleInfo::from_id_latest("abstract:ibc-client")?, None)],
                }],
            })?,
            None,
        )?;
        let remote_acc_tx = result;
        // @feedback chain id or chain name?
        interchain.wait_ibc(&home_chain_id, remote_acc_tx)?;

        remote_proxies = list_remote_proxies(&archway, &archway_acc)?;
        println!("archway proxies: {:?}", remote_proxies);
    } else {
        println!("juno>archway>osmosis exists")
    }

    // Get the archway remote hosts
    let osmosis_account_id = AccountId::new(
        home_acc.id()?.seq(),
        AccountTrace::Remote(vec![
            ChainName::from_str(HOME_CHAIN_NAME)?,
            ChainName::from_str(FIRST_HOP_CHAIN_NAME)?,
        ]),
    )?;
    let osmosis_acc = AbstractAccount::new(
        &Abstract::load_from(osmosis.clone())?,
        osmosis_account_id.clone(),
    );

    // Check whether juno>archway>osmosis has IBC enabled
    if !osmosis_acc.manager.is_module_installed(IBC_CLIENT_ID)? {
        println!("Enabling IBC on Osmosis");
        let home_manager = &home_acc.manager;
        let enable_ibc_tx = home_manager.execute_on_remote_module(
            FIRST_HOP_CHAIN_NAME.into(),
            PROXY,
            to_json_binary(&abstract_core::proxy::ExecuteMsg::IbcAction {
                msgs: vec![abstract_core::ibc_client::ExecuteMsg::RemoteAction {
                    host_chain: SECOND_HOP_CHAIN_NAME.to_string(),
                    action: HostAction::Dispatch {
                        manager_msg: manager::ExecuteMsg::UpdateSettings {
                            ibc_enabled: Some(true),
                        },
                    },
                    callback_info: None,
                }],
            })?,
            None,
        )?;
        interchain.wait_ibc(&home_chain_id, enable_ibc_tx)?;
    } else {
        println!("Ibc client is installed on Osmosis!");
    }

    let mut remote_proxies = list_remote_proxies(&osmosis, &osmosis_acc)?;
    println!("osmosis remote_proxies: {:?}", remote_proxies);

    // check whether juno>archway>osmosis has registered juno
    if remote_proxies
        .iter()
        .find(|(chain, _)| chain == &ChainName::from_str(HOME_CHAIN_NAME).unwrap())
        .is_none()
    {
        println!("Registering remote account from osmosis on juno");
        let home_manager = &home_acc.manager;
        let result = home_manager.execute_on_remote_module(
            FIRST_HOP_CHAIN_NAME.into(),
            PROXY,
            to_json_binary(&abstract_core::proxy::ExecuteMsg::IbcAction {
                msgs: vec![abstract_core::ibc_client::ExecuteMsg::RemoteAction {
                    host_chain: SECOND_HOP_CHAIN_NAME.to_string(),
                    action: HostAction::Dispatch {
                        manager_msg: manager::ExecuteMsg::ExecOnModule {
                            exec_msg: to_json_binary(
                                &abstract_core::proxy::ExecuteMsg::IbcAction {
                                    msgs: vec![abstract_core::ibc_client::ExecuteMsg::Register {
                                        host_chain: THIRD_HOP_CHAIN_NAME.into(),
                                        base_asset: None,
                                        namespace: None,
                                        install_modules: vec![ModuleInstallConfig::new(
                                            ModuleInfo::from_id_latest("abstract:ibc-client")?,
                                            None,
                                        )],
                                    }],
                                },
                            )?,
                            module_id: PROXY.to_string(),
                        },
                    },
                    callback_info: None,
                }],
            })?,
            None,
        )?;
        let remote_acc_tx = result;
        // @feedback chain id or chain name?
        interchain.wait_ibc(&home_chain_id, remote_acc_tx)?;

        let mut remote_proxies = list_remote_proxies(&osmosis, &osmosis_acc)?;
        println!("accounts: {:?}", remote_proxies);
    } else {
        println!("juno>archway>osmosis>juno exists!!!!");
    }

    // Currently send funds, send back
    // maybe Send juno, swap juno for osmo, send back?

    Ok(())
}

// This script aims to test a theory that we can make executing messages on Archway cheap by doing them all over IBC
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
