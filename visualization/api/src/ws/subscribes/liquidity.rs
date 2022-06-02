use serde::Deserialize;

#[derive(Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiquiditySubscription {
    pub exchange_id: String,
    pub currency_pair: String,
}
