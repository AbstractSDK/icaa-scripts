use std::str::FromStr;

use abstract_client::{AbstractClient, Namespace};
use abstract_dex_adapter::msg::DexExecuteMsg::RawAction;
use abstract_dex_adapter::msg::{DexAnsAction, DexRawAction};
use abstract_dex_adapter::DEX_ADAPTER_ID;
use abstract_interface::{Abstract, AbstractAccount, ManagerExecFns};
use abstract_std as abstract_core;
use abstract_std::objects::AnsAsset;
use abstract_std::{ibc_client, ibc_host::{HelperAction, HostAction}, manager, manager::ModuleInstallConfig, objects::{
    chain_name::ChainName, gov_type::GovernanceDetails, module::ModuleInfo, AccountId,
    AssetEntry,
}, PROXY, proxy};
use cosmwasm_std::{coins, to_json_binary, Uint128};
use cw_asset::AssetInfo;
use cw_orch_interchain::prelude::{ChannelCreationValidator, DaemonInterchainEnv, InterchainEnv};
use cw_orch::{
    contract::Deploy, daemon::networks::parse_network, daemon::queriers::Bank,
    environment::BankQuerier, prelude::*,
};
use log::warn;
use pretty_env_logger::env_logger;
use tokio::runtime::Runtime;

use icaa_scripts::{press_enter_to_continue, ABSTRACT_DEX_ADAPTER_ID, IBC_CLIENT_ID, JUNO_1};

const HOME_CHAIN_ID: &str = "juno-1";
const HOME_CHAIN_NAME: &str = "juno";
const HOME_CHAIN_BASE_ASSET: &str = "juno>juno";
const REMOTE_CHAIN_ID: &str = "osmosis-1";
const REMOTE_CHAIN_NAME: &str = "osmosis";
const REMOTE_CHAIN_BASE_ASSET: &str = "osmosis>osmo";

const HOME_DEX_NAME: &str = "wyndex";
const REMOTE_DEX_NAME: &str = "osmosis";

fn icaa_demo() -> anyhow::Result<()> {
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
    let bank = home.bank_querier();
    let balance = bank.balance(sender.clone(), Some(home_denom.to_string()))?;
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
        .name("ICAA PL Test")
        // @feedback: this namespace method should note that remote namespaces will not work
        .namespace(Namespace::new("icaa-test-juno-osmosis-3")?)
        .sub_account(&parent_account_client)
        .build()?;

    let home_acc = AbstractAccount::new(&home_abstr, home_account_client.id()?);

    press_enter_to_continue();

    // Check and enable IBC on home chain
    // @feedback module installation check should be available on Abstract Client
    // AND should be able to check IBC status (get_ibc_status)
    if !home_acc.manager.is_module_installed(IBC_CLIENT_ID)? {
        warn!("Enabling IBC on {}", HOME_CHAIN_NAME);
        // @feedback include whether it errors if not enabled
        home_account_client.set_ibc_status(true)?;
    } else {
        warn!("IBC is already enabled on {}!", HOME_CHAIN_NAME);
    }

    press_enter_to_continue();

    // could sanity check that osmosis is an available host
    // let remote_hosts = ibc_client.list_remote_hosts()?.hosts;
    // warn!("hosts: {:?}", remote_hosts);

    // CHeck for and register remote account on osmosis
    // @feedback should be able to get remote account IDs (or list of remote chains)
    // should be able to get remote accounts
    // get_remote_accounts: Vec<Account<Daemon>>
    warn!("Checking for remote accounts on {}", REMOTE_CHAIN_NAME);
    let mut remote_proxies = icaa_scripts::list_remote_proxies(&home, &home_acc)?;


    if !remote_proxies
        .iter()
        .any(|(chain, _)| chain == &ChainName::from_str(REMOTE_CHAIN_NAME).unwrap())
    {
        warn!("Registering remote account on {}", REMOTE_CHAIN_NAME);

        // @feedback on below: we should be able to customize the remote account, should be named: `create_remote_account`, ensure doc comment specifies what happens if exists
        // let remote_acc_tx = home_acc.register_remote_account(REMOTE_CHAIN_NAME)?;

        // thought: maybe we could do with account builder (like sub_account method)
        // Ex: let remote_acc = home_client.account_builder().remote_account(home_account_client)

        // @feedback - wrong doc comment, also should be named `create_remote_account`
        // parent_account_client.create_ibc_account()
        let remote_acc_tx = home_account_client.execute(
            vec![&abstract_core::proxy::ExecuteMsg::IbcAction {
                msg: abstract_core::ibc_client::ExecuteMsg::Register {
                    host_chain: REMOTE_CHAIN_NAME.into(),
                    base_asset: Some(AssetEntry::from(REMOTE_CHAIN_BASE_ASSET)),
                    namespace: None,
                    install_modules: vec![ModuleInstallConfig::new(
                        ModuleInfo::from_id_latest(ABSTRACT_DEX_ADAPTER_ID)?,
                        None,
                    )],
                },
            }.into()?],
            &[],
        )?;
        // @feedback chain id or chain name?
        interchain.wait_ibc(&HOME_CHAIN_ID, remote_acc_tx)?;

        remote_proxies = icaa_scripts::list_remote_proxies(&home, &home_acc)?;
        warn!("remote_proxies: {:?}", remote_proxies);
    } else {
        warn!(
            "{} already registered on {}",
            REMOTE_CHAIN_NAME, HOME_CHAIN_NAME
        );
    }

    press_enter_to_continue();

    // Check home account balance before sending
    // Check both balances
    let home_balances = home_account_client.query_balances()?;
    warn!("Home balances before sending: {:?}", home_balances);

    let remote_balances = get_remote_balances(&remote, &home_acc)?;
    warn!("Remote balances before sending: {:?}", remote_balances);

    let mut home_base_denom_balance = home_account_client.query_balance(home_denom)?;
    if home_base_denom_balance.is_zero() {
        warn!("Sending some funds from wallet to account.");
        // @feedback make it easier to send funds from wallet?
        //  - maybe acc_client.deposit() method
        let _bank_send_tx = rt.block_on(home.daemon.sender.bank_send(
            // @feedback: home_account_client.address() to get the address of the proxy?
            home_account_client.proxy()?.as_str(),
            coins(500, home_denom),
        ))?;

        home_base_denom_balance = home_account_client.query_balance(home_denom)?;
    }

    press_enter_to_continue();

    // Send funds to the remote account
    warn!(
        "Sending funds from {} to {}.",
        HOME_CHAIN_ID, REMOTE_CHAIN_NAME
    );
    // @feedback: should be able to send_funds_to_remote
    let send_funds_tx = home_acc.manager.execute_on_module(
        PROXY,
        abstract_core::proxy::ExecuteMsg::IbcAction {
            msg: abstract_core::ibc_client::ExecuteMsg::SendFunds {
                host_chain: REMOTE_CHAIN_NAME.into(),
                funds: coins(home_base_denom_balance.u128(), home_denom),
            },
        },
    )?;
    interchain.wait_ibc(&HOME_CHAIN_ID, send_funds_tx)?;

    // Check both balances
    let home_balance = home_account_client.query_balance(home_denom)?;
    warn!("Home balance after sending: {:?}", home_balance);

    let remote_balance = get_remote_balance(&remote, &home_acc)?;
    warn!("Remote balance after sending: {:?}", remote_balance);

    press_enter_to_continue();

    // Swap the home funds for the remote funds and then send them back
    warn!(
        "Attempting to swap {} {} for {} using {} dex on {}!",
        remote_balance,
        HOME_CHAIN_BASE_ASSET,
        REMOTE_CHAIN_BASE_ASSET,
        REMOTE_DEX_NAME,
        REMOTE_CHAIN_NAME
    );
    let swap_tx = home_acc.manager.execute_on_remote_module(
        REMOTE_CHAIN_NAME,
        ABSTRACT_DEX_ADAPTER_ID,
        to_json_binary(
            &(Into::<abstract_dex_adapter::msg::ExecuteMsg>::into(
                abstract_dex_adapter::msg::DexExecuteMsg::AnsAction {
                    dex: REMOTE_DEX_NAME.into(),
                    action: DexAnsAction::Swap {
                        offer_asset: AnsAsset {
                            name: AssetEntry::from(HOME_CHAIN_BASE_ASSET),
                            amount: remote_balance,
                        },
                        ask_asset: AssetEntry::from(REMOTE_CHAIN_BASE_ASSET),
                        max_spread: None,
                        belief_price: None,
                    },
                },
            )),
        )?,
        // None,
    )?;
    interchain.wait_ibc(&HOME_CHAIN_ID, swap_tx)?;

    warn!(
        "Successfully swapped assets using {}'s dex on {}!",
        REMOTE_CHAIN_NAME, REMOTE_DEX_NAME
    );

    press_enter_to_continue();

    // send funds back
    // @feedback revised: send_remote_funds(remote_chain_name, funds, memo)
    // @feedback: should be able to request_remote_funds
    warn!("Requesting all funds back");
    let send_funds_tx = home_acc.manager.execute_on_module(
        PROXY,
        abstract_core::proxy::ExecuteMsg::IbcAction {
            msg: abstract_core::ibc_client::ExecuteMsg::RemoteAction {
                host_chain: REMOTE_CHAIN_NAME.into(),
                action: HostAction::Helpers(HelperAction::SendAllBack),
            },
        },
    )?;

    // @feedback we should have a helper for exec_on_proxy(msg: proxy::ExecuteMsg)
    let _manual_send_funds_tx = home_acc.manager.execute_on_module(
        PROXY,
        abstract_core::proxy::ExecuteMsg::IbcAction {
            msg: abstract_core::ibc_client::ExecuteMsg::RemoteAction {
                host_chain: REMOTE_CHAIN_NAME.into(),
                action: HostAction::Dispatch {
                    manager_msgs: vec![manager::ExecuteMsg::ExecOnModule {
                        module_id: PROXY.to_string(),
                        exec_msg: to_json_binary(&proxy::ExecuteMsg::IbcAction {
                            msg: ibc_client::ExecuteMsg::SendFunds {
                                host_chain: "noble".to_string(),
                                funds: vec![],
                                // memo: Some(json!({
                                //   "forward": {
                                //     "receiver": "juno_proxy",
                                //     "port": "transfer",
                                //     "channel": "channel-noble-"
                                //   }
                                // }).to_string())
                            },
                        })?,
                    }],
                },
            },
        },
    )?;

    interchain.wait_ibc(&HOME_CHAIN_ID, send_funds_tx)?;

    let home_balances = home_account_client.query_balances()?;
    warn!("Home balances after receiving back: {:?}", home_balances);

    let remote_base_denom = match home_abstr
        .ans_host
        .resolve(&AssetEntry::from(REMOTE_CHAIN_BASE_ASSET))?
    {
        AssetInfo::Native(denom) => denom,
        _ => anyhow::bail!("remote base asset is not a token"),
    };
    let home_remote_denom_balance = home_account_client.query_balance(remote_base_denom)?;
    warn!(
        "Home balance after sending: {:?}",
        home_remote_denom_balance
    );

    // Check and enable dex adapter on home chain
    if !home_acc.manager.is_module_installed(DEX_ADAPTER_ID)? {
        warn!("Enabling dex adapter on {}", HOME_CHAIN_NAME);
        home_acc
            .manager
            .install_module::<Empty>(DEX_ADAPTER_ID, None, None)?;
    } else {
        warn!("Dex adapter is already installed on {}!", HOME_CHAIN_NAME);
    };

    press_enter_to_continue();

    warn!(
        "Swapping {} {} for {} using {} dex on {}!",
        home_remote_denom_balance,
        REMOTE_CHAIN_BASE_ASSET,
        HOME_CHAIN_BASE_ASSET,
        HOME_DEX_NAME,
        HOME_CHAIN_NAME
    );

    home_acc.manager.execute_on_module(
        DEX_ADAPTER_ID,
        Into::<abstract_dex_adapter::msg::ExecuteMsg>::into(
            abstract_dex_adapter::msg::DexExecuteMsg::AnsAction {
                dex: HOME_DEX_NAME.into(),
                action: DexAnsAction::Swap {
                    offer_asset: AnsAsset {
                        name: AssetEntry::from(REMOTE_CHAIN_BASE_ASSET),
                        amount: home_remote_denom_balance,
                    },
                    ask_asset: AssetEntry::from(HOME_CHAIN_BASE_ASSET),
                    max_spread: None,
                    belief_price: None,
                },
            },
        ),
    )?;

    // Check home account balance before sending
    // Check both balances
    let home_balances = home_account_client.query_balances()?;
    warn!("Home balances after everything: {:?}", home_balances);

    Ok(())
}

fn get_remote_balances(
    remote: &Daemon,
    home_acc: &AbstractAccount<Daemon>,
) -> anyhow::Result<Vec<Coin>> {
    let remote_account_id = AccountId::remote(
        home_acc.id()?.seq(),
        vec![ChainName::from_str(HOME_CHAIN_NAME)?],
    )?;
    let remote_acc = AbstractAccount::new(
        &Abstract::load_from(remote.clone())?,
        remote_account_id.clone(),
    );
    let remote_balances = remote
        .bank_querier()
        .balance(remote_acc.proxy.address()?, None)?;
    println!("Remote balances: {:?}", remote_balances);
    Ok(remote_balances)
}

fn get_remote_balance(
    remote: &Daemon,
    home_acc: &AbstractAccount<Daemon>,
) -> anyhow::Result<Uint128> {
    let remote_account_id = AccountId::remote(
        home_acc.id()?.seq(),
        vec![ChainName::from_str(HOME_CHAIN_NAME)?],
    )?;
    // WE use the abstractAccount here because the ABstractClient can't load a remote account? Not sure

    let remote_acc = AbstractAccount::new(
        &Abstract::load_from(remote.clone())?,
        remote_account_id.clone(),
    );

    // @feedback: should be able to do something like:
    // remote_acc.balances()
    // remote_acc.balance(X)
    // remote_acc.ans_balance(ansX)

    let remote_balances = remote
        .bank_querier()
        .balance(remote_acc.proxy.address()?, None)?;
    println!("Remote balances: {:?}", remote_balances);

    let remote_abstr = Abstract::load_from(remote.clone())?;
    let juno_info = remote_abstr
        .ans_host
        .resolve(&AssetEntry::from(HOME_CHAIN_BASE_ASSET))?;
    let juno_denom = match juno_info {
        AssetInfo::Native(address) => address,
        _ => anyhow::bail!("juno is not a token"),
    };

    let remote_balance = remote
        .bank_querier()
        .balance(remote_acc.proxy.address()?, Some(juno_denom))?[0]
        .amount;

    Ok(remote_balance)
}

fn main() {
    dotenv().ok();
    env_logger::init();

    use dotenv::dotenv;

    if let Err(ref err) = icaa_demo() {
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
       let register_base_asset_tx = home_acc.manager.execute_on_remotow e_module(REMOTE_CHAIN_NAME, PROXY, to_json_binary(&abstract_core::proxy::ExecuteMsg::UpdateAssets {
           to_add: vec![(AssetEntry::from(HOME_CHAIN_BASE_ASSET), UncheckedPriceSource::None)],
           to_remove: vec![],
       })?, None)?;
       interchain.wait_ibc(&HOME_CHAIN_ID, register_base_asset_tx)?;

   // Check and enable dex adapter on home chain
   let _dex_adapter: Application<Daemon, DexAdapter<Daemon>> = if !home_acc.manager.is_module_installed(DEX_ADAPTER_ID)? {
       warn!("Enabling dex adapter on {}", HOME_CHAIN_NAME);
       home_account_client.install_adapter(&[])?
   } else {
       warn!("IBC is already enabled on {}!", HOME_CHAIN_NAME);
       Application::new(home_account_client, DexAdapter::new(DEX_ADAPTER_ID, home.clone()))?
   };
*/
