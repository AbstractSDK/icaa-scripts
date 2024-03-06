use std::str::FromStr;

use abstract_client::{AbstractClient, Environment, Namespace};

use abstract_core::ibc_host::{HelperAction, HostAction};

use abstract_core::objects::chain_name::ChainName;
use abstract_core::objects::gov_type::GovernanceDetails;
use abstract_core::objects::{AccountId, AssetEntry};
use abstract_core::objects::account::AccountTrace;
use abstract_core::objects::salt::generate_instantiate_salt;

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

const HOME_CHAIN_ID: &str = "osmosis-1";
const HOME_CHAIN_NAME: &str = "osmosis";
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

    // Setup abstract + home account
    let home_abstr = Abstract::load_from(home.clone())?;
    let home_client = AbstractClient::new(home.clone()).unwrap();
    // let home_account_client = home_client
    //     .account_builder()
    //     .account_id(48)
    //     .build()?;

    let id = AccountId::new(48, AccountTrace::Local)?;
    println!("id: {:?}", id);
    let salt = generate_instantiate_salt(&id);
    println!("salt: {:?}", salt);
    let wasm_querier = home_client.environment().wasm_querier();
    let creator = home_abstr.module_factory.addr_str()?;
    println!("creator: {:?}", creator);
    let code_id = home_client.version_control().get_module_code_id(
        "abstract:carrot-app",
        abstract_core::objects::module::ModuleVersion::Latest,
    )?;

    let code_id_hash = wasm_querier.code_id_hash(code_id)?;
    println!("code_id_hash: {:?}", code_id_hash);
    let addr = wasm_querier
        .instantiate2_addr(code_id, creator, salt)?;
    let init2 = Addr::unchecked(addr);
    println!("init2: {:?}", init2);

    press_enter_to_continue();

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

/*
       // Regiser base asset on remote
       let register_base_asset_tx = home_acc.manager.execute_on_remote_module(REMOTE_CHAIN_NAME, PROXY, to_json_binary(&abstract_core::proxy::ExecuteMsg::UpdateAssets {
           to_add: vec![(AssetEntry::from("juno>juno"), UncheckedPriceSource::None)],
           to_remove: vec![],
       })?, None)?;
       interchain.wait_ibc(&HOME_CHAIN_ID.into(), register_base_asset_tx)?;

*/
