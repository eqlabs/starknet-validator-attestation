use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message, RequestBuilderExt};
use starknet::core::types::{BlockTag, Felt};
use url::Url;

#[derive(Debug, serde::Serialize)]
pub struct SubscriptionRequest {
    #[serde(rename = "jsonrpc")]
    pub _jsonrpc: JsonRpcVersion,
    pub id: u64,
    #[serde(flatten)]
    pub method: SubscriptionMethod,
}

#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(tag = "method", content = "params")]
pub enum SubscriptionMethod {
    #[serde(rename = "starknet_subscribeNewHeads")]
    SubscribeNewHeads {},
    #[serde(rename = "starknet_subscribeEvents")]
    SubscribeEvents {
        #[serde(skip_serializing_if = "Option::is_none")]
        from_address: Option<Felt>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        keys: Vec<Vec<Felt>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        block_id: Option<BlockTag>,
    },
}

#[derive(Debug, serde::Deserialize)]
struct SubscriptionResponse {
    #[serde(rename = "jsonrpc")]
    pub _jsonrpc: JsonRpcVersion,
    pub result: u64,
    pub id: u64,
}

#[derive(Debug, PartialEq, serde::Deserialize)]
pub struct SubscriptionNotification {
    #[serde(rename = "jsonrpc")]
    pub _jsonrpc: JsonRpcVersion,
    #[serde(flatten)]
    pub method: NotificationMethod,
}

#[derive(Debug, PartialEq, serde::Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum NotificationMethod {
    #[serde(rename = "starknet_subscriptionNewHeads")]
    NewHeads(NewHeadsNotificationParams),
    #[serde(rename = "starknet_subscriptionEvents")]
    Events(EventsNotificationParams),
    #[serde(rename = "starknet_subscriptionReorg")]
    Reorg(ReorgNotificationParams),
}

#[derive(Debug, PartialEq, serde::Deserialize)]
pub struct NewHeadsNotificationParams {
    pub result: NewHeader,
    pub subscription_id: u64,
}

#[derive(Debug, PartialEq, serde::Deserialize)]
pub struct NewHeader {
    pub block_hash: Felt,
    pub block_number: u64,
}

#[derive(Debug, PartialEq, serde::Deserialize)]
pub struct EventsNotificationParams {
    pub result: EmittedEvent,
    pub subscription_id: u64,
}

#[derive(Debug, PartialEq, serde::Deserialize)]
pub struct EmittedEvent {
    pub from_address: Felt,
    pub keys: Vec<Felt>,
    pub data: Vec<Felt>,
    // Pending events do not have block hash
    pub block_hash: Option<Felt>,
    pub block_number: u64,
    pub transaction_hash: Felt,
}

#[derive(Debug, PartialEq, serde::Deserialize)]
pub struct ReorgNotificationParams {
    pub result: ReorgData,
    pub subscription_id: u64,
}

#[derive(Debug, PartialEq, serde::Deserialize)]
pub struct ReorgData {
    pub starting_block_number: u64,
    pub ending_block_number: u64,
    // There are other fields here in the notification that we're ignoring.
}

#[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum JsonRpcVersion {
    #[serde(rename = "2.0")]
    V2_0,
}

pub async fn subscribe(
    url: Url,
    subscription: SubscriptionMethod,
) -> anyhow::Result<(reqwest_websocket::WebSocket, u64)> {
    let mut client = reqwest::Client::default()
        .get(url.clone())
        .upgrade()
        .send()
        .await
        .context("Connecting to JSON-RPC API")?
        .into_websocket()
        .await
        .context("Upgrading to websocket connection")?;

    let request_id = 1;

    client
        .send(
            Message::text_from_json(&SubscriptionRequest {
                _jsonrpc: JsonRpcVersion::V2_0,
                id: request_id,
                method: subscription,
            })
            .context("Failed to create JSON subscription request")?,
        )
        .await
        .context("Sending subscription request")?;

    let response: SubscriptionResponse = client
        .next()
        .await
        .context("Receiving subscription response")??
        .json()
        .context("Parsing subscription response")?;

    if response.id != request_id {
        return Err(anyhow::anyhow!(
            "Unexpected subscription response ID: expected {}, got {}",
            request_id,
            response.id
        ));
    }

    Ok((client, response.result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use starknet::macros::felt;

    #[test]
    fn test_new_heads_subscription_request() {
        let request = SubscriptionRequest {
            _jsonrpc: JsonRpcVersion::V2_0,
            method: SubscriptionMethod::SubscribeNewHeads {},
            id: 1,
        };

        let expected = json!({
            "jsonrpc": "2.0",
            "method": "starknet_subscribeNewHeads",
            "params": {},
            "id": 1
        });

        let serialized = serde_json::to_value(request).unwrap();
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_events_subscription_request() {
        let request = SubscriptionRequest {
            _jsonrpc: JsonRpcVersion::V2_0,
            method: SubscriptionMethod::SubscribeEvents {
                from_address: Some(felt!("0xdeadbeef")),
                keys: vec![],
                block_id: None,
            },
            id: 1,
        };

        let expected = json!({
            "jsonrpc": "2.0",
            "method": "starknet_subscribeEvents",
            "params": {
                "from_address": "0xdeadbeef",
            },
            "id": 1
        });

        let serialized = serde_json::to_value(request).unwrap();
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_new_heads_notification() {
        let expected = SubscriptionNotification {
            _jsonrpc: JsonRpcVersion::V2_0,
            method: NotificationMethod::NewHeads(NewHeadsNotificationParams {
                result: NewHeader {
                    block_hash: Felt::ZERO,
                    block_number: 0,
                },
                subscription_id: 0,
            }),
        };
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "starknet_subscriptionNewHeads",
            "params": {
                "result": {
                    "block_hash": "0x0",
                    "block_number": 0,
                    "l1_da_mode":"CALLDATA"
                },
                "subscription_id": 0
            }
        });

        let deserialized: SubscriptionNotification = serde_json::from_value(notification).unwrap();
        assert_eq!(deserialized, expected);
    }

    #[test]
    fn test_events_notification() {
        let expected = SubscriptionNotification {
            _jsonrpc: JsonRpcVersion::V2_0,
            method: NotificationMethod::Events(EventsNotificationParams {
                result: EmittedEvent {
                    from_address: felt!("0xdeadbeef"),
                    keys: vec![felt!("0x2"), felt!("0x3")],
                    data: vec![felt!("0x0"), felt!("0x1")],
                    block_hash: Some(Felt::ZERO),
                    block_number: 0,
                    transaction_hash: felt!("0x4"),
                },
                subscription_id: 0,
            }),
        };
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "starknet_subscriptionEvents",
            "params": {
                "result": {
                    "block_hash": "0x0",
                    "block_number": 0,
                    "data": ["0x0", "0x1"],
                    "from_address": "0xdeadbeef",
                    "keys": ["0x2", "0x3"],
                    "transaction_hash": "0x4",
                },
                "subscription_id": 0
            }
        });

        let deserialized: SubscriptionNotification = serde_json::from_value(notification).unwrap();
        assert_eq!(deserialized, expected);
    }

    #[test]
    fn test_reorg_notification() {
        let expected = SubscriptionNotification {
            _jsonrpc: JsonRpcVersion::V2_0,
            method: NotificationMethod::Reorg(ReorgNotificationParams {
                result: ReorgData {
                    starting_block_number: 20,
                    ending_block_number: 30,
                },
                subscription_id: 0,
            }),
        };
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "starknet_subscriptionReorg",
            "params": {
                "result": {
                    "starting_block_number": 20,
                    "starting_block_hash": "0xdeadbeef",
                    "ending_block_number": 30,
                    "ending_block_hash": "0xbeefdead"
                },
                "subscription_id": 0
            }
        });

        let deserialized: SubscriptionNotification = serde_json::from_value(notification).unwrap();
        assert_eq!(deserialized, expected);
    }
}
