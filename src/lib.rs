mod pb;
use hex_literal::hex;
use pb::transfer::v1 as transfer;

use substreams::{
    log::{self, info},
    store::{
        StoreAdd, StoreAddInt64, StoreGet, StoreGetProto, StoreNew, StoreSet, StoreSetIfNotExists,
        StoreSetIfNotExistsProto,
    },
    Hex,
};
use substreams_entity_change::pb::entity::EntityChanges;
use substreams_entity_change::tables::Tables as EntityChangesTables;
use substreams_ethereum::pb::eth::v2::Block;

#[allow(unused_imports)]
use num_traits::cast::ToPrimitive;

use num_bigint::BigInt;

substreams_ethereum::init!();
//find all erc721 transfers in a block
//get all receipt logs for the transfer
//check if the logs have trades
// save transfers
//if the transfer is a trade save the trade

//log.address is the collection address
const SEAPORT_TOPIC0: [u8; 32] =
    hex!("9d9af8e38d66c62e2c12f0225249fd9d721c54b83f48d9352c97c6cacdcb6f31");

const WYVERN_TOPIC0: [u8; 32] =
    hex!("c4109843e0b7d514e4c093114b863f8e7d8d9a458c372cd51bfe526b588006c9");

const ERC721_TRANSFER_TOPIC0: [u8; 32] =
    hex!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");

const WETH_ADDRESS: [u8; 20] = hex!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");

const NULL_ADDRESS: [u8; 20] = hex!("0000000000000000000000000000000000000000");

pub struct TradeUtils;

impl TradeUtils {
    pub fn get_address_from_hex_string(hex_string: &str) -> String {
        hex_string[24..].to_string()
    }

    pub fn hex_to_bigint(hex: &str) -> BigInt {
        let hex = hex.to_uppercase();
        let mut result = BigInt::from(0);
        let mut power = BigInt::from(1);

        for c in hex.chars().rev() {
            let value = match c {
                '0'..='9' => c as u8 - b'0',
                'A'..='F' => c as u8 - b'A' + 10,
                _ => continue,
            };
            result += &power * BigInt::from(value);
            power *= BigInt::from(16);
        }

        result
    }
}

// Maps
#[substreams::handlers::map]
fn map_transfers(blk: Block) -> transfer::Transfers {
    let mut data = transfer::Transfers::default();
    blk.transaction_traces.iter().for_each(|tx| {
        tx.receipt.iter().for_each(|r| {
            r.logs.iter().for_each(|l| {
                if l.topics.is_empty() {
                    return;
                }

                let event_signature = &l.topics[0];
                let topics_length = l.topics.len();
                if event_signature.as_slice() == ERC721_TRANSFER_TOPIC0 && topics_length == 4 {
                    let mut transfer_event = transfer::Transfer::default();

                    // Initialize all fields to ensure proper protobuf encoding
                    transfer_event.evt_tx_hash = Hex(&tx.hash).to_string();
                    transfer_event.evt_index = tx.index as u32;
                    transfer_event.evt_block_time = Some(blk.timestamp().to_owned());
                    transfer_event.evt_block_number = blk.number.to_u64().unwrap();
                    transfer_event.from = Hex(&l.topics[1]).to_string();
                    transfer_event.to = Hex(&l.topics[2]).to_string();
                    transfer_event.collection_address = Hex(&l.address).to_string();
                    transfer_event.token_id = Hex(&l.topics[3]).to_string();
                    transfer_event.is_burned = false;
                    transfer_event.is_minted = false;
                    transfer_event.is_traded = false;
                    transfer_event.market = 0;
                    transfer_event.ordinal = l.ordinal as u64;
                    // Check for mint/burn
                    if transfer_event.from
                        == "0000000000000000000000000000000000000000000000000000000000000000"
                    {
                        transfer_event.is_minted = true;
                    } else if transfer_event.to
                        == "0000000000000000000000000000000000000000000000000000000000000000"
                    {
                        transfer_event.is_burned = true;
                    }

                    transfer_event.transfer_logs = r
                        .logs
                        .iter()
                        .map(|l| transfer::TransferLog {
                            address: Hex(&l.address).to_string(),
                            data: Hex(&l.data).to_string(),
                            topics: l.topics.iter().map(|t| Hex(t).to_string()).collect(),
                        })
                        .collect();

                    // Check if this transfer is a trade
                    for log in &r.logs {
                        if !log.topics.is_empty() {
                            let topic0 = &log.topics[0];
                            if topic0.as_slice() == SEAPORT_TOPIC0 {
                                transfer_event.is_traded = true;
                                transfer_event.market = 2;
                                break;
                            } else if topic0.as_slice() == WYVERN_TOPIC0 {
                                transfer_event.is_traded = true;
                                transfer_event.market = 1;
                                break;
                            } else {
                                transfer_event.is_traded = false;
                            }
                        }
                    }
                    data.transfers.push(transfer_event);
                }
            })
        });
    });

    data
}

#[substreams::handlers::map]
fn map_trades(
    transfers: transfer::Transfers,
) -> Result<transfer::Trades, substreams::errors::Error> {
    // Create a default trade in case no valid trade is found
    let mut default_trade = transfer::Trade::default();
    let mut trades = transfer::Trades::default();
    // Check if we have any transfers to process
    if transfers.transfers.is_empty() {
        return Ok(trades);
    }

    // Process the first transfer (assuming one transfer per call)
    for transfer in &transfers.transfers {
        log::info!("Processing transfer for trade detection:");
        log::info!("  Is traded: {}", transfer.is_traded);
        log::info!("  From: {}", transfer.from);
        log::info!("  To: {}", transfer.to);
        log::info!("  Collection: {}", transfer.collection_address);
        log::info!("  Token ID: {}", transfer.token_id);
        log::info!("  Market: {:?}", transfer.market);

        // If not a trade, return default trade with basic info
        if !transfer.is_traded {
            log::info!("Transfer is not a trade, skipping");
            default_trade.id = format!("{}-{}", transfer.evt_tx_hash, transfer.evt_index);
            default_trade.hash = transfer.evt_tx_hash.clone();
            continue;
        }

        // Process transfer logs
        for (i, log) in transfer.transfer_logs.iter().enumerate() {
            log::info!(
                "Processing log {} of {}",
                i + 1,
                transfer.transfer_logs.len()
            );

            if log.topics.len() < 2 {
                continue;
            }

            let event_signature = &log.topics[0];
            log::info!("  Event signature: {}", event_signature);
            let topic1 = &log.topics[1];
            let data = &log.data;

            // Process Seaport trades
            if event_signature == &Hex(SEAPORT_TOPIC0).to_string() {
                log::info!("Processing Seaport trade");
                if let Some(trade) = process_seaport_trade(transfer, log, topic1, data) {
                    log::info!("Seaport trade processed successfully");
                    trades.trades.push(trade);
                }
            }
            // Process Wyvern trades
            else if event_signature == &Hex(WYVERN_TOPIC0).to_string() {
                log::info!("Processing Wyvern trade");
                if let Some(trade) = process_wyvern_trade(transfer, log, topic1, data) {
                    log::info!("Wyvern trade processed successfully");
                    trades.trades.push(trade);
                }
            }
        }
    }
    // If no trade was processed, return default trade with basic info
    Ok(trades)
}

#[substreams::handlers::map]
fn map_collections(transfers: transfer::Transfers) -> transfer::Collections {
    let mut collections = transfer::Collections::default();
    for transfer in &transfers.transfers {
        let mut collection = transfer::Collection::default();
        collection.id = transfer.collection_address.clone();
        collection.token_count = 0;
        collection.owner_count = 0;
        collection.event_count = 0;
        collection.creation_block = transfer.evt_block_number;
        collection.creation_timestamp = transfer.evt_block_time.as_ref().unwrap().seconds as u64;
        collections.collections.push(collection);
    }
    collections
}

#[substreams::handlers::map]
fn map_erc20s(trades: transfer::Trades) -> transfer::Erc20s {
    let mut erc20s = transfer::Erc20s::default();
    for trade in &trades.trades {
        let mut erc20 = transfer::Erc20::default();
        erc20.id = trade.erc20_token_address.clone();
        erc20.address = trade.erc20_token_address.clone();
        erc20s.erc20s.push(erc20);
    }
    erc20s
}

#[substreams::handlers::map]
fn map_tokens(transfers: transfer::Transfers) -> transfer::Tokens {
    let mut tokens = transfer::Tokens::default();
    for transfer in &transfers.transfers {
        let mut token = transfer::Token::default();
        //token id is the collection address and token id
        token.id = format!("{}-{}", transfer.collection_address, transfer.token_id);
        token.collection_address = transfer.collection_address.clone();
        token.token_id = transfer.token_id.clone();
        token.owner = transfer.to.clone();
        token.mint_timestamp = transfer.evt_block_time.as_ref().unwrap().seconds as u64;
        tokens.tokens.push(token);
    }
    tokens
}

#[substreams::handlers::map]
fn map_accounts(transfers: transfer::Transfers) -> transfer::Accounts {
    let mut accounts = transfer::Accounts::default();
    for transfer in &transfers.transfers {
        // Create account for sender
        // Skip creating sender account if it's a mint
        // Skip creating receiver account if it's a burn
        if !transfer.is_minted {
            let mut from_account = transfer::Account::default();
            from_account.id = transfer.from.clone();
            from_account.address = transfer.from.clone();
            from_account.token_count = 0;
            accounts.accounts.push(from_account);
            continue;
        }

        if !transfer.is_burned {
            let mut to_account = transfer::Account::default();
            to_account.id = transfer.to.clone();
            to_account.address = transfer.to.clone();
            to_account.token_count = 0;
            accounts.accounts.push(to_account);
            continue;
        }
        let mut from_account = transfer::Account::default();
        from_account.id = transfer.from.clone();
        from_account.address = transfer.from.clone();
        from_account.token_count = 0;
        accounts.accounts.push(from_account);

        // Create account for receiver
        let mut to_account = transfer::Account::default();
        to_account.id = transfer.to.clone();
        to_account.address = transfer.to.clone();
        to_account.token_count = 0;
        accounts.accounts.push(to_account);
    }
    accounts
}

// Stores

#[substreams::handlers::store]
fn store_collections(
    collections: transfer::Collections,
    store: StoreSetIfNotExistsProto<transfer::Collection>,
) {
    for collection in collections.collections {
        store.set_if_not_exists(0, &collection.id, &collection);
        log::info!("Attempted to store new collection: {}", collection.id);
    }
}

#[substreams::handlers::store]
fn store_erc20s(erc20s: transfer::Erc20s, store: StoreSetIfNotExistsProto<transfer::Erc20>) {
    for erc20 in erc20s.erc20s {
        store.set_if_not_exists(0, &erc20.id, &erc20);
        log::info!("Attempted to store new ERC20: {}", erc20.id);
    }
}

#[substreams::handlers::store]
fn store_tokens(tokens: transfer::Tokens, store: StoreSetIfNotExistsProto<transfer::Token>) {
    for token in tokens.tokens {
        store.set_if_not_exists(0, &token.id, &token);
        log::info!("Attempted to store new token: {}", token.id);
    }
}

#[substreams::handlers::store]
fn store_accounts(
    accounts: transfer::Accounts,
    store: StoreSetIfNotExistsProto<transfer::Account>,
) {
    for account in accounts.accounts {
        store.set_if_not_exists(0, &account.id, &account);
        log::info!("Attempted to store new account: {}", account.id);
    }
}

#[substreams::handlers::store]
fn store_collection_token_count(transfers: transfer::Transfers, store: StoreAddInt64) {
    for transfer in transfers.transfers {
        if transfer.is_minted {
            // Increment token count by 1 for new mints
            store.add(
                transfer.ordinal,
                &format!("collection_token_count:{}", &transfer.collection_address),
                1,
            );
            log::info!(
                "Incremented token count for collection: {}",
                transfer.collection_address
            );
        } //add burn
    }
}

#[substreams::handlers::store]
fn store_collection_owner_count(transfers: transfer::Transfers, store: StoreAddInt64) {
    for transfer in transfers.transfers {
        if transfer.is_minted {
            store.add(0, format!("collection:{}", transfer.collection_address), 1);
        }
    }
}

#[substreams::handlers::store]
fn store_collection_event_count(transfers: transfer::Transfers, store: StoreAddInt64) {
    for transfer in transfers.transfers {
        // Increment event count by 1 for any transfer event
        store.add(
            transfer.ordinal,
            &format!("collection_event_count:{}", &transfer.collection_address),
            1,
        );
        log::info!(
            "Incremented event count for collection: {}",
            transfer.collection_address
        );
    }
}

// Helpler function to process Seaport trades
fn process_seaport_trade(
    transfer: &transfer::Transfer,
    log: &transfer::TransferLog,
    topic1: &str,
    data: &str,
) -> Option<transfer::Trade> {
    log::info!("data: {}", data);
    log::info!("data len: {}", data.len());
    if data.len() < 896 {
        return None;
    }
    log::info!("data len 2: {}", data.len());
    let from = &transfer.from;
    let to = &transfer.to;
    log::info!("from: {}", from);
    log::info!("to: {}", to);
    log::info!("topic1: {}", topic1);
    if &topic1 == &from {
        let amount_paid = TradeUtils::hex_to_bigint(&data[832..896]);
        let marketplace_address = log.address.clone();
        let mut fee = BigInt::from(0);
        let to_address_from_receipt = TradeUtils::get_address_from_hex_string(&data[64..128]);
        let formatted_to = to.clone()[24..].to_string();
        log::info!("to_address_from_receipt: {}", to_address_from_receipt);
        log::info!("to: {}", formatted_to);
        if &to_address_from_receipt != &formatted_to {
            return None;
        }

        if data.len() > 1152 {
            fee = TradeUtils::hex_to_bigint(&data[1152..1216]);
        }
        let mut token_address = TradeUtils::get_address_from_hex_string(&data[704..768]);
        if token_address == Hex(&NULL_ADDRESS).to_string() {
            token_address = Hex(&WETH_ADDRESS).to_string();
        }

        Some(build_trade(
            transfer,
            log,
            &token_address,
            &amount_paid.to_string(),
            "Seaport",
            &marketplace_address,
            &fee,
        ))
    } else if &topic1 == &to {
        let amount_paid = TradeUtils::hex_to_bigint(&data[832..896]);
        let marketplace_address = log.address.clone();
        let mut fee = BigInt::from(0);
        let from_address_from_receipt = TradeUtils::get_address_from_hex_string(&data[64..128]);
        let formatted_from = from.clone()[24..].to_string();
        log::info!("from_address_from_receipt: {}", from_address_from_receipt);
        log::info!("from: {}", formatted_from);
        if &from_address_from_receipt != &formatted_from {
            return None;
        }
        if data.len() > 1152 {
            fee = TradeUtils::hex_to_bigint(&data[1152..1216]);
        }
        let mut token_address = TradeUtils::get_address_from_hex_string(&data[704..768]);
        if token_address == Hex(&NULL_ADDRESS).to_string() {
            token_address = Hex(&WETH_ADDRESS).to_string();
        }

        Some(build_trade(
            transfer,
            log,
            &token_address,
            &amount_paid.to_string(),
            "Seaport",
            &marketplace_address,
            &fee,
        ))
    } else {
        None
    }
}

// Helper function to process Wyvern trades
fn process_wyvern_trade(
    transfer: &transfer::Transfer,
    log: &transfer::TransferLog,
    topic1: &str,
    data: &str,
) -> Option<transfer::Trade> {
    if data.len() < 256 || log.topics.len() < 3 {
        return None;
    }

    let from = TradeUtils::get_address_from_hex_string(&transfer.from);
    let to = TradeUtils::get_address_from_hex_string(&transfer.to);
    let seller = TradeUtils::get_address_from_hex_string(topic1);

    if seller == from {
        let amount_paid = TradeUtils::hex_to_bigint(&data[64..128]);
        let marketplace_address = log.address.clone();
        let token_address = Hex(&WETH_ADDRESS).to_string();

        Some(build_trade(
            transfer,
            log,
            &token_address,
            &amount_paid.to_string(),
            "Wyvern",
            &marketplace_address,
            &BigInt::from(0),
        ))
    } else {
        None
    }
}

fn build_trade(
    transfer: &transfer::Transfer,
    log: &transfer::TransferLog,
    erc20_address: &str,
    amount: &str,
    marketplace_name: &str,
    marketplace_address: &str,
    fee: &BigInt,
) -> transfer::Trade {
    let mut trade = transfer::Trade::default();
    trade.id = format!("{}-{}", transfer.evt_tx_hash, transfer.evt_index);
    trade.hash = transfer.evt_tx_hash.clone();
    trade.block_number = transfer.evt_block_number;
    trade.timestamp = transfer.evt_block_time.as_ref().unwrap().seconds as u64;
    trade.collection_address = transfer.collection_address.clone();
    trade.token_id = transfer.token_id.clone();
    trade.erc20_token_address = erc20_address.to_string();
    trade.erc20_token_amount = amount.to_string();
    trade.marketplace_address = marketplace_address.to_string();
    trade.marketplace_name = marketplace_name.to_string();
    trade.fee = fee.to_u64().unwrap();
    trade.ordinal = transfer.ordinal;
    trade
}

// Add the graph output module for Substreams-powered Subgraph
#[substreams::handlers::map]
fn graph_out(
    transfers: transfer::Transfers,
    trades: transfer::Trades,
    collections_store: StoreGetProto<transfer::Collection>,
    tokens_store: StoreGetProto<transfer::Token>,
    erc20s_store: StoreGetProto<transfer::Erc20>,
    accounts_store: StoreGetProto<transfer::Account>,
) -> Result<EntityChanges, substreams::errors::Error> {
    log::info!("=== GRAPH_OUT FUNCTION CALLED ===");
    log::info!("Transfers count: {}", transfers.transfers.len());
    log::info!("Trades count: {}", trades.trades.len());

    // Initialize Database Changes container
    let mut tables = EntityChangesTables::new();
    let mut entity_count = 0;

    // STEP 1: Create all Collection entities first
    log::info!("STEP 1: Creating Collection entities");
    let mut processed_collections = std::collections::HashSet::new();

    for transfer in &transfers.transfers {
        if !processed_collections.contains(&transfer.collection_address) {
            if let Some(collection) = collections_store.get_last(&transfer.collection_address) {
                log::info!(
                    "Creating Collection entity: {}",
                    transfer.collection_address
                );
                tables
                    .create_row("Collection", &transfer.collection_address)
                    .set("tokenCount", collection.token_count)
                    .set("ownerCount", collection.owner_count)
                    .set("eventCount", collection.event_count)
                    .set("creationTimestamp", collection.creation_timestamp)
                    .set("creationBlock", collection.creation_block)
                    .set("baseURI", "")
                    .set("name", "")
                    .set("symbol", "");
            } else {
                log::info!(
                    "No collection data found, creating minimal Collection: {}",
                    transfer.collection_address
                );
                tables
                    .create_row("Collection", &transfer.collection_address)
                    .set("tokenCount", 0i64)
                    .set("ownerCount", 0i64)
                    .set("eventCount", 0i64)
                    .set(
                        "creationTimestamp",
                        transfer.evt_block_time.as_ref().unwrap().seconds,
                    )
                    .set("creationBlock", transfer.evt_block_number)
                    .set("baseURI", "")
                    .set("name", "")
                    .set("symbol", "");
            }
            processed_collections.insert(transfer.collection_address.clone());
            entity_count += 1;
        }
    }

    // STEP 2: Create all Account entities
    log::info!("STEP 2: Creating Account entities");
    let mut processed_accounts = std::collections::HashSet::new();

    for transfer in &transfers.transfers {
        // Create FROM account (if not mint)
        if !transfer.is_minted && !processed_accounts.contains(&transfer.from) {
            if let Some(from_account) = accounts_store.get_last(&transfer.from) {
                log::info!("Creating FROM Account entity: {}", transfer.from);
                tables
                    .create_row("Account", &transfer.from)
                    .set("tokenCount", from_account.token_count);
            } else {
                log::info!("Creating minimal FROM Account entity: {}", transfer.from);
                tables
                    .create_row("Account", &transfer.from)
                    .set("tokenCount", 0i64);
            }
            processed_accounts.insert(transfer.from.clone());
            entity_count += 1;
        }

        // Create TO account (if not burn)
        if !transfer.is_burned && !processed_accounts.contains(&transfer.to) {
            if let Some(to_account) = accounts_store.get_last(&transfer.to) {
                log::info!("Creating TO Account entity: {}", transfer.to);
                tables
                    .create_row("Account", &transfer.to)
                    .set("tokenCount", to_account.token_count);
            } else {
                log::info!("Creating minimal TO Account entity: {}", transfer.to);
                tables
                    .create_row("Account", &transfer.to)
                    .set("tokenCount", 0i64);
            }
            processed_accounts.insert(transfer.to.clone());
            entity_count += 1;
        }
    }

    // STEP 3: Create all ERC20 entities from trades
    log::info!("STEP 3: Creating ERC20 entities");
    let mut processed_erc20s = std::collections::HashSet::new();

    for trade in &trades.trades {
        if !trade.erc20_token_address.is_empty()
            && !processed_erc20s.contains(&trade.erc20_token_address)
        {
            log::info!("Creating ERC20 entity: {}", trade.erc20_token_address);
            tables.create_row("ERC20", &trade.erc20_token_address);
            processed_erc20s.insert(trade.erc20_token_address.clone());
            entity_count += 1;
        }
    }

    // STEP 4: Create Token entities
    log::info!("STEP 4: Creating Token entities");
    let mut processed_tokens = std::collections::HashSet::new();

    for transfer in &transfers.transfers {
        let token_id = format!("{}-{}", transfer.collection_address, transfer.token_id);

        if transfer.is_minted && !processed_tokens.contains(&token_id) {
            log::info!("Creating Token entity: {}", token_id);

            // Parse tokenId as BigInt
            let token_id_bigint = if let Ok(parsed) = transfer.token_id.parse::<i64>() {
                parsed
            } else {
                // If parsing fails, try to convert hex to decimal
                if transfer.token_id.starts_with("0x") || transfer.token_id.len() > 10 {
                    // For hex or very large numbers, use a hash or truncated version
                    (transfer.token_id.len() as i64) // Fallback
                } else {
                    0i64
                }
            };

            tables
                .create_row("Token", &token_id)
                .set("collection", &transfer.collection_address) // Entity reference
                .set("tokenId", token_id_bigint) // Use BigInt
                .set("owner", &transfer.to) // Entity reference
                .set(
                    "mintTimestamp",
                    transfer.evt_block_time.as_ref().unwrap().seconds,
                )
                .set("tokenUri", ""); // Add missing optional field

            processed_tokens.insert(token_id.clone());
            entity_count += 1;
        } else if !transfer.is_minted {
            log::info!("Updating Token entity owner: {}", token_id);
            tables
                .update_row("Token", &token_id)
                .set("owner", &transfer.to); // Entity reference
        }
    }

    // STEP 5: Create TokenEvent entities
    log::info!("STEP 5: Creating TokenEvent entities");
    for (i, transfer) in transfers.transfers.iter().enumerate() {
        let token_event_id = format!(
            "{}-{}-{}",
            transfer.collection_address, transfer.evt_tx_hash, transfer.evt_index
        );

        log::info!("Creating TokenEvent {}: ID={}", i + 1, token_event_id);

        // Parse tokenId as BigInt
        let token_id_bigint = if let Ok(parsed) = transfer.token_id.parse::<i64>() {
            parsed
        } else {
            if transfer.token_id.starts_with("0x") || transfer.token_id.len() > 10 {
                (transfer.token_id.len() as i64) // Fallback
            } else {
                0i64
            }
        };

        tables
            .create_row("TokenEvent", token_event_id)
            .set("hash", &transfer.evt_tx_hash)
            .set(
                "type",
                if transfer.is_minted {
                    "MINT"
                } else if transfer.is_burned {
                    "BURN"
                } else {
                    "TRANSFER"
                },
            )
            .set("isTrade", transfer.is_traded)
            .set("logIndex", transfer.evt_index as i32)
            .set("collection", &transfer.collection_address) // Entity reference
            .set("tokenId", token_id_bigint) // Use BigInt
            .set("to", &transfer.to) // Entity reference
            .set("from", &transfer.from) // Entity reference
            .set("blockNumber", transfer.evt_block_number)
            .set(
                "timestamp",
                transfer.evt_block_time.as_ref().unwrap().seconds,
            )
            .set("nonce", 0);

        entity_count += 1;
    }

    // STEP 6: Create Trade entities
    log::info!("STEP 6: Creating Trade entities");
    for (i, trade) in trades.trades.iter().enumerate() {
        if trade.id.is_empty() {
            continue;
        }

        log::info!("Creating Trade {}: ID={}", i + 1, trade.id);

        // Parse erc20TokenAmount as BigInt
        let erc20_amount_bigint = if let Ok(parsed) = trade.erc20_token_amount.parse::<i64>() {
            parsed
        } else {
            0i64 // Fallback for parsing errors
        };

        let token_entity_id = format!("{}-{}", trade.collection_address, trade.token_id);

        tables
            .create_row("Trade", &trade.id)
            .set("hash", &trade.hash)
            .set("blockNumber", trade.block_number)
            .set("timestamp", trade.timestamp)
            .set("collection", &trade.collection_address) // Entity reference
            .set("token", &token_entity_id) // Entity reference
            .set("erc20TokenAmount", erc20_amount_bigint) // Use BigInt
            .set("erc20Token", &trade.erc20_token_address) // Entity reference
            .set("marketplaceAddress", &trade.marketplace_address)
            .set("marketplaceName", &trade.marketplace_name);

        entity_count += 1;
    }

    let entity_changes = tables.to_entity_changes();
    log::info!("=== GRAPH_OUT SUMMARY ===");
    log::info!("Total entities created/updated: {}", entity_count);
    log::info!(
        "EntityChanges generated: {} changes",
        entity_changes.entity_changes.len()
    );

    Ok(entity_changes)
}
