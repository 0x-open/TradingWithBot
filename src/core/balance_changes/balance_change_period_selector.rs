use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use chrono::Duration;
use itertools::Itertools;
use mockall_double::double;
use parking_lot::Mutex;

#[double]
use crate::core::balance_manager::balance_manager::BalanceManager;
#[double]
use crate::core::misc::time_manager::time_manager;

use crate::core::{
    balance_changes::profit_loss_balance_change::ProfitLossBalanceChange,
    balance_manager::position_change::PositionChange, exchanges::common::TradePlaceAccount,
    DateTime,
};

pub(crate) struct BalanceChangePeriodSelector {
    pub(super) period: Duration,
    balance_manager: Option<Arc<Mutex<BalanceManager>>>,
    balance_changes_queues_by_trade_place:
        HashMap<TradePlaceAccount, VecDeque<ProfitLossBalanceChange>>,
}

impl BalanceChangePeriodSelector {
    pub fn new(
        period: Duration,
        balance_manager: Option<Arc<Mutex<BalanceManager>>>,
    ) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            period,
            balance_manager,
            balance_changes_queues_by_trade_place: HashMap::new(),
        }))
    }

    pub fn add(&mut self, balance_change: &ProfitLossBalanceChange) {
        log::info!(
            "Balance changes enqueue: {} {} {}",
            balance_change.change_date,
            balance_change.currency_code,
            balance_change.balance_change
        );

        let trade_place = TradePlaceAccount::new(
            balance_change.exchange_account_id.clone(),
            balance_change.currency_pair.clone(),
        );

        self.balance_changes_queues_by_trade_place
            .entry(trade_place.clone())
            .or_default()
            .push_back(balance_change.clone());

        self.synchronize_period(balance_change.change_date, &trade_place);
    }

    fn synchronize_period(
        &mut self,
        now: DateTime,
        trade_place: &TradePlaceAccount,
    ) -> Option<PositionChange> {
        let start_of_period = now - self.period;

        let balance_changes_queue = match self
            .balance_changes_queues_by_trade_place
            .get_mut(trade_place)
        {
            Some(balance_changes_queue) => balance_changes_queue,
            None => {
                log::error!("Can't find queue for trade place {:?}", trade_place);
                return None;
            }
        };

        let position_change_before_period = match &self.balance_manager {
            Some(balance_manager) => {
                let position_change = balance_manager
                    .lock()
                    .get_last_position_change_before_period(trade_place, start_of_period);

                log::info!(
                    "Balance changes list {} {:?}",
                    start_of_period,
                    position_change
                );
                position_change
            }
            None => {
                // keep all items for web
                log::info!(
                    "Balance changes list {} position_change is None",
                    start_of_period,
                );
                None
            }
        };

        while let Some(last_change) = balance_changes_queue.front() {
            if position_change_before_period.is_none() && last_change.change_date >= start_of_period
                || position_change_before_period.is_some()
                    && last_change.client_order_fill_id
                        == position_change_before_period
                            .clone()
                            .expect("position_change_before_period can't be None here")
                            .client_order_fill_id
            {
                break;
            }

            log::info!(
                "Balance changes dequeue {} {} {}",
                last_change.change_date,
                last_change.currency_code,
                last_change.balance_change
            );
            let _ = balance_changes_queue.pop_front();
        }
        position_change_before_period
    }

    pub fn get_items(&mut self) -> Vec<Vec<ProfitLossBalanceChange>> {
        self.balance_changes_queues_by_trade_place
            .clone()
            .keys()
            .map(|current_trade_place| self.get_items_by_trade_place(current_trade_place))
            .collect_vec()
    }

    pub fn get_items_by_trade_place(
        &mut self,
        trade_place: &TradePlaceAccount,
    ) -> Vec<ProfitLossBalanceChange> {
        let position_change = self.synchronize_period(time_manager::now(), trade_place);

        let balance_changes_queue = self
            .balance_changes_queues_by_trade_place
            .get(trade_place)
            .expect("failed to get balance changes queue by trade_place");

        balance_changes_queue
            .iter()
            .map(|x| {
                if let Some(position_change) = &position_change {
                    if x.client_order_fill_id == position_change.client_order_fill_id {
                        return x.with_portion(position_change.portion);
                    }
                }
                x.clone()
            })
            .collect_vec()
    }
}
