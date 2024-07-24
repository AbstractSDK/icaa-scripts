use std::str::FromStr;

use abstract_client::{AbstractClient, Namespace};
use abstract_interface::{Abstract, AbstractAccount};
use abstract_std::{
    objects::{
        chain_name::ChainName, AccountId,
    }, ibc_host::HostAction, PROXY, proxy, ibc_client, manager,
};
use abstract_std::objects::module::ModuleVersion;
use cosmwasm_std::{to_json_binary, wasm_execute};
use cw_orch_interchain::prelude::{ChannelCreationValidator, DaemonInterchainEnv, InterchainEnv};
use cw_orch::{
    contract::Deploy, daemon::networks::parse_network,
    environment::BankQuerier, prelude::*,
};
use cw_orch::environment::{ChainKind, NetworkInfo};
use log::warn;
use pretty_env_logger::env_logger;
use tokio::runtime::Runtime;
use cw721_base::ExecuteMsg as NftExecuteMsg;

use icaa_scripts::{IBC_CLIENT_ID};

pub const XION_NETWORK: NetworkInfo = NetworkInfo {
    chain_name: "xion",
    pub_address_prefix: "xion",
    coin_type: 118u32,
};

pub const XION_TESTNET_1: ChainInfo = ChainInfo {
    kind: ChainKind::Testnet,
    chain_id: "xion-testnet-1",
    gas_denom: "uxion",
    gas_price: 0.0,
    grpc_urls: &["http://xion-testnet-grpc.polkachu.com:22390"],
    network_info: cw_orch::daemon::networks::xion::XION_NETWORK,
    lcd_url: None,
    fcd_url: None,
};

const HOME_CHAIN_ID: &str = XION_TESTNET_1.chain_id;
const HOME_CHAIN_NAME: &str = XION_NETWORK.chain_name;
const REMOTE_CHAIN_ID: &str = "pion-1";
const REMOTE_CHAIN_NAME: &str = "pion";
const REMOTE_NFT_ADDR: &str = "neutron1d2s4ss5k5wqntnv7zj65q5wj67sjfedvn6wzpr82mqatuksdk6cqjqatcc";
fn icaa_demo() -> anyhow::Result<()> {
    let rt = Runtime::new()?;

    // Setup interchain environment
    let home_chain_info = XION_TESTNET_1; // let home_chain_info = parse_network(HOME_CHAIN_ID).unwrap();
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
        // @uno-feedback: Which namespace method? You mean we should note that you can't have sub-account from different chain?
        .sub_account(&parent_account_client)
        .build()?;

    let home_acc = AbstractAccount::new(&home_abstr, home_account_client.id()?);


    // Check and enable IBC on home chain
    // @feedback✅ module installation check should be available on Abstract Client
    // AND should be able to check IBC status (get_ibc_status)
    if !home_acc.manager.is_module_installed(IBC_CLIENT_ID)? {
        warn!("Enabling IBC on {}", HOME_CHAIN_NAME);
        // @feedback✅ include whether it errors if not enabled
        home_account_client.as_ref().manager.install_module_version::<Empty>("abstract:ibc-client", ModuleVersion::Latest, None, None)?;
    } else {
        warn!("IBC is already enabled on {}!", HOME_CHAIN_NAME);
    }

    // could sanity check that osmosis is an available host
    // let remote_hosts = ibc_client.list_remote_hosts()?.hosts;
    // warn!("hosts: {:?}", remote_hosts);

    // CHeck for and register remote account on osmosis
    // @feedback✅ should be able to get remote account IDs (or list of remote chains)
    // should be able to get remote accounts
    // get_remote_accounts: Vec<Account<Daemon>>
    warn!("Checking for remote accounts on {}", REMOTE_CHAIN_NAME);
    let mut remote_proxies = icaa_scripts::list_remote_proxies(&home, &home_acc)?;


    if !remote_proxies
        .iter()
        .any(|(chain, _)| chain == &ChainName::from_str(REMOTE_CHAIN_NAME).unwrap())
    {
        warn!("Registering remote account on {}", REMOTE_CHAIN_NAME);

        // @feedback✅ on below: we should be able to customize the remote account, should be named: `create_remote_account`, ensure doc comment specifies what happens if exists
        // let remote_acc_tx = home_acc.register_remote_account(REMOTE_CHAIN_NAME)?;

        // thought: maybe we could do with account builder (like sub_account method)
        // Ex: let remote_acc = home_client.account_builder().remote_account(home_account_client)

        // @feedback✅ - wrong doc comment, also should be named `create_remote_account`
        // parent_account_client.create_ibc_account()
        let remote_acc_tx = home_account_client.create_ibc_account(
            REMOTE_CHAIN_NAME,
            None,
            None,
            vec![]
        )?;
        // @feedback chain id or chain name?
        let ibc_resp = interchain.wait_ibc(HOME_CHAIN_ID, remote_acc_tx)?;

        // match &ibc_resp.packets[0].outcome {
        //     cw_orch_interchain::types::IbcPacketOutcome::Timeout { .. } => {
        //         panic!("Timeout!")
        //     }
        //     cw_orch_interchain::types::IbcPacketOutcome::Success { ack, .. } => match ack {
        //         cw_orch_interchain::types::IbcPacketAckDecode::Error(e) => {
        //             panic!("Expected a success ack not a error ack: {:?}", e)
        //         }
        //         cw_orch_interchain::types::IbcPacketAckDecode::Success(_) => {
        //         }
        //         cw_orch_interchain::types::IbcPacketAckDecode::NotParsed(original_ack) => {
        //             panic!("Not parsed")
        //         }
        //     },
        // }
        remote_proxies = icaa_scripts::list_remote_proxies(&home, &home_acc)?;
        warn!("remote_proxies: {:?}", remote_proxies);
    } else {
        warn!(
            "{} already registered on {}",
            REMOTE_CHAIN_NAME, HOME_CHAIN_NAME
        );
    }

    let remote_nft_tx = home_account_client.as_ref().manager.execute_on_module(
        PROXY,
        proxy::ExecuteMsg::IbcAction {
            msg: ibc_client::ExecuteMsg::RemoteAction {
                host_chain: REMOTE_CHAIN_NAME.to_string(),
                action: HostAction::Dispatch {
                    manager_msgs: vec![manager::ExecuteMsg::ExecOnModule {
                        module_id: PROXY.to_string(),
                        exec_msg: to_json_binary(&proxy::ExecuteMsg::ModuleAction {
                            msgs: vec![wasm_execute(
                                REMOTE_NFT_ADDR,
                                &NftExecuteMsg::<Option<Empty>, Empty>::Mint {
                                    token_id: "disregarded".to_string(),
                                    owner: "disregarded".to_string(),
                                    token_uri: None,
                                    extension: None,
                                },
                                vec![],
                            )?
                            .into()],
                        })?,
                    }],
                },
            },
        },
    )?;

    interchain.wait_ibc(HOME_CHAIN_ID, remote_nft_tx)?;

    println!("Minted nft");


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
