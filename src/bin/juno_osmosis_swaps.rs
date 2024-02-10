use std::str::FromStr;

use abstract_client::{AbstractClient, Namespace};

use abstract_core::ibc_host::{HelperAction, HostAction};
use abstract_core::manager::ModuleInstallConfig;
use abstract_core::objects::{AccountId, AssetEntry};
use abstract_core::objects::chain_name::ChainName;
use abstract_core::objects::gov_type::GovernanceDetails;
use abstract_core::objects::module::ModuleInfo;
use abstract_core::PROXY;
use abstract_dex_adapter::msg::{DexAction, OfferAsset};
use abstract_interface::{Abstract, AbstractAccount, ManagerExecFns};
use cosmwasm_std::{coins, to_json_binary, Uint128};
use cw_asset::AssetInfo;
use cw_orch::daemon::networks::parse_network;
use cw_orch::daemon::queriers::Bank;
use cw_orch::{contract::Deploy, prelude::*};
use cw_orch::environment::BankQuerier;
use cw_orch_interchain::prelude::{ChannelCreationValidator, DaemonInterchainEnv, InterchainEnv};
use icaa_scripts::{ABSTRACT_DEX_ADAPTER_ID, IBC_CLIENT_ID, JUNO_1, press_enter_to_continue};
use pretty_env_logger::env_logger;
use tokio::runtime::Runtime;

use log::warn;

const HOME_CHAIN_ID: &str = "juno-1";
const HOME_CHAIN_NAME: &str = "juno";
const HOME_CHAIN_BASE_ASSET: &'static str = "juno>juno";
const REMOTE_CHAIN_ID: &str = "osmosis-1";
const REMOTE_CHAIN_NAME: &str = "osmosis";
const REMOTE_CHAIN_BASE_ASSET: &str = "osmosis>osmo";

const REMOTE_DEX_NAME: &'static str = "osmosis";

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
    let parent_account_client = home_client
        .account_builder()
        .namespace(Namespace::new("icaa-test-2")?)
        .build()?;

    // Execute the test on the sub-account
    let home_account_client = home_client
        .account_builder()
        .name("ICAA Test Juno Osmosis Archway (test)")
        .namespace(Namespace::new("icaa-test-juno-osmosis-archway-test")?)
        .ownership(GovernanceDetails::SubAccount {
            proxy: parent_account_client.proxy()?.into(),
            manager: parent_account_client.manager()?.into(),
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
    warn!("Checking for remote accounts on {}", REMOTE_CHAIN_NAME);
    let mut remote_proxies = icaa_scripts::list_remote_proxies(&home, &home_acc)?;

    if !remote_proxies
        .iter()
        .any(|(chain, _)| chain == &ChainName::from_str(REMOTE_CHAIN_NAME).unwrap())
    {
        warn!("Registering remote account on {}", REMOTE_CHAIN_NAME);
        // let remote_acc_tx = home_acc.register_remote_account(REMOTE_CHAIN_NAME)?;
        let remote_acc_tx = home_acc.manager.exec_on_module(
            to_json_binary(&abstract_core::proxy::ExecuteMsg::IbcAction {
                msgs: vec![abstract_core::ibc_client::ExecuteMsg::Register {
                    host_chain: REMOTE_CHAIN_NAME.into(),
                    base_asset: Some(AssetEntry::from(REMOTE_CHAIN_BASE_ASSET)),
                    namespace: None,
                    install_modules: vec![ModuleInstallConfig::new(
                        ModuleInfo::from_id_latest(ABSTRACT_DEX_ADAPTER_ID)?,
                        None,
                    )],
                }],
            })?,
            PROXY.to_string(),
            &[],
        )?;
        // @feedback chain id or chain name?
        interchain.wait_ibc(&HOME_CHAIN_ID.into(), remote_acc_tx)?;

        remote_proxies = icaa_scripts::list_remote_proxies(&home, &home_acc)?;
        warn!("remote_proxies: {:?}", remote_proxies);
    } else {
        warn!("{} already registered on {}", REMOTE_CHAIN_NAME, HOME_CHAIN_NAME);
    }

    press_enter_to_continue();

    // Check home account balance before sending
    // Check both balances
    let home_balances = home_account_client.query_balances()?;
    warn!("Home balances before sending: {:?}", home_balances);

    let remote_balances = get_remote_balances(&remote, &home_acc)?;
    warn!("Remote balances before sending: {:?}", remote_balances);

    let home_base_denom_balance = home_account_client.query_balance(home_denom)?;
    if home_base_denom_balance.is_zero() {
        warn!("Sending some funds from wallet to account.");
        // @feedback make it easier to send funds from wallet?
        //  - maybe a acc_client.deposit() method
        let bank_send_tx = rt.block_on(home.daemon.sender.bank_send(
            home_account_client.proxy()?.as_str(),
            coins(500, home_denom),
        ))?;
        interchain.wait_ibc(&HOME_CHAIN_ID.into(), bank_send_tx)?;
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
                funds: coins(home_base_denom_balance.u128(), home_denom),
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

    // Swap the home funds for the remote funds and then send them back
    warn!("Attempting to swap {} {} for {} using {} dex on {}!", remote_balance, HOME_CHAIN_BASE_ASSET, REMOTE_CHAIN_BASE_ASSET, REMOTE_DEX_NAME,REMOTE_CHAIN_NAME);
    let swap_tx = home_acc.manager.execute_on_remote_module(
        REMOTE_CHAIN_NAME,
        ABSTRACT_DEX_ADAPTER_ID,
        to_json_binary(&(Into::<abstract_dex_adapter::msg::ExecuteMsg>::into(abstract_dex_adapter::msg::DexExecuteMsg::Action {
            dex: REMOTE_DEX_NAME.into(),
            action: DexAction::Swap {
                offer_asset: OfferAsset {
                    name: AssetEntry::from(HOME_CHAIN_BASE_ASSET),
                    amount: remote_balance.into(),
                },
                ask_asset: AssetEntry::from(REMOTE_CHAIN_BASE_ASSET),
                max_spread: None,
                belief_price: None,
            }
        })))?,
        None
    )?;
    interchain.wait_ibc(&HOME_CHAIN_ID.into(), swap_tx)?;

    warn!("Successfully swapped assets using {}'s dex on {}!", REMOTE_CHAIN_NAME, REMOTE_DEX_NAME);

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

    let home_balances = home_account_client.query_balances()?;
    warn!("Home balances after receiving back: {:?}", home_balances);

    // Currently send funds, send back
    // maybe Send juno, swap juno for osmo, send back?

    Ok(())
}

fn get_remote_balances(remote: &Daemon, home_acc: &AbstractAccount<Daemon>) -> anyhow::Result<Vec<Coin>> {
    let remote_account_id = AccountId::remote(
        home_acc.id()?.seq(),
        vec![ChainName::from_str(HOME_CHAIN_NAME)?]
    )?;
    let remote_acc = AbstractAccount::new(
        &Abstract::load_from(remote.clone())?,
        remote_account_id.clone(),
    );
    let remote_balances = remote.balance(remote_acc.proxy.address()?, None)?;
    println!("Remote balances: {:?}", remote_balances);
    Ok(remote_balances)
}



fn get_remote_balance(remote: &Daemon, home_acc: &AbstractAccount<Daemon>) -> anyhow::Result<Uint128> {
    let remote_account_id = AccountId::remote(
        home_acc.id()?.seq(),
        vec![ChainName::from_str(HOME_CHAIN_NAME)?]
    )?;
    let remote_acc = AbstractAccount::new(
        &Abstract::load_from(remote.clone())?,
        remote_account_id.clone(),
    );
    let remote_balances = remote.balance(remote_acc.proxy.address()?, None)?;
    println!("Remote balances: {:?}", remote_balances);

    let remote_abstr = Abstract::load_from(remote.clone())?;
    let juno_info = remote_abstr.ans_host.resolve(&AssetEntry::from(HOME_CHAIN_BASE_ASSET))?;
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
            to_add: vec![(AssetEntry::from(HOME_CHAIN_BASE_ASSET), UncheckedPriceSource::None)],
            to_remove: vec![],
        })?, None)?;
        interchain.wait_ibc(&HOME_CHAIN_ID.into(), register_base_asset_tx)?;

 */
