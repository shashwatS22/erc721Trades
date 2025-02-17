mod pb;
use hex_literal::hex;
use pb::transfer::v1 as transfer;

use substreams::Hex;
use substreams_ethereum::pb::eth::v2::Block;

#[allow(unused_imports)]
use num_traits::cast::ToPrimitive;

use num_bigint::BigInt;
use std::str::FromStr;

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
        format!("0x{}", &hex_string[24..])
    }

    pub fn get_address_from_bytes(bytes: &[u8]) -> String {
        format!("0x{}", &Hex(bytes).to_string()[26..])
    }

    pub fn hex_to_bigint(hex: &str) -> BigInt {
        let hex = hex.trim_start_matches("0x").to_uppercase();
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

#[substreams::handlers::map]
fn map_my_data(blk: Block) -> transfer::Transfer {
    let mut data = transfer::Transfer::default();

    blk.transaction_traces.iter().for_each(|tx| {
        tx.receipt.iter().for_each(|r| {
            r.logs.iter().for_each(|l| {
                // Check if there are any topics before accessing
                if l.topics.is_empty() {
                    return;
                }

                let event_signature = &l.topics[0];
                let topics_length = l.topics.len();
                if event_signature.as_slice() == ERC721_TRANSFER_TOPIC0 {
                    if topics_length == 4 {
                        let from = Hex(&l.topics[1]).to_string();
                        let to = Hex(&l.topics[2]).to_string();
                        let token_id = Hex(&l.topics[3]).to_string();
                        data.token_address = Hex(&l.address).to_string();
                        if from == Hex(&NULL_ADDRESS).to_string() {
                            data.is_minted = true;
                        } else if to == Hex(&NULL_ADDRESS).to_string() {
                            data.is_burned = true;
                        }
                        data.from = from;
                        data.to = to;
                        data.token_id = token_id;
                        data.evt_tx_hash = Hex(&tx.hash).to_string();
                        data.evt_index = tx.index as u32;
                        data.evt_block_time = Some(blk.timestamp().to_owned());
                        data.evt_block_number = blk.number.to_u64().unwrap();
                        data.transfer_logs = r
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
                                    data.is_traded = true;
                                    data.market = transfer::Market::Opensea as i32;
                                    break;
                                } else if topic0.as_slice() == WYVERN_TOPIC0 {
                                    data.is_traded = true;
                                    data.market = transfer::Market::Wyvern as i32;
                                    break;
                                }
                            }
                        }
                    }
                }
            })
        })
    });

    data
}

#[substreams::handlers::map]
fn map_trades(transfer: transfer::Transfer) -> Result<transfer::Trade, substreams::errors::Error> {
    let mut trades = transfer::Trade::default();
    if transfer.is_traded {
        
    }
    for log in transfer.transfer_logs.iter() {
        if let Some(topic0) = log.topics.get(0) {
            if transfer.is_traded {
                let data = log.data.trim_start_matches("0x");
                if data.len() < 512 {
                    continue;
                }

                // Extract payment details from the first consideration item
                let consideration_offset = 320;
                let consideration_data = &data[consideration_offset..];

                // Get the token address and amount from the consideration
                let token_address =
                    TradeUtils::get_address_from_hex_string(&consideration_data[24..64]);

                let amount = TradeUtils::hex_to_bigint(&consideration_data[128..192]);

                let mut erc20_address = token_address;
                if erc20_address == format!("0x{}", Hex(&NULL_ADDRESS).to_string()) {
                    erc20_address = format!("0x{}", Hex(&WETH_ADDRESS).to_string());
                }

                return Ok(build_trade(
                    &transfer,
                    &log,
                    &erc20_address,
                    &amount.to_string(),
                    if transfer.market == transfer::Market::Opensea as i32 {
                        "OpenSea"
                    } else {
                        "Wyvern"
                    },
                ));
            }
        }
    }

    Ok(trades)
}

fn build_trade(
    transfer: &transfer::Transfer,
    log: &transfer::TransferLog,
    erc20_address: &str,
    amount: &str,
    marketplace_name: &str,
) -> transfer::Trade {
    let mut trade = transfer::Trade::default();
    trade.id = format!("{}-{}", transfer.evt_tx_hash, transfer.evt_index);
    trade.hash = transfer.evt_tx_hash.clone();
    trade.block_number = transfer.evt_block_number;
    trade.timestamp = transfer.evt_block_time.as_ref().unwrap().seconds as u64;
    trade.collection_address = transfer.token_address.clone();
    trade.token_id = transfer.token_id.clone();
    trade.erc20_token_address = erc20_address.to_string();
    trade.erc20_token_amount = amount.to_string();
    trade.marketplace_address = log.address.clone();
    trade.marketplace_name = marketplace_name.to_string();
    trade
}
