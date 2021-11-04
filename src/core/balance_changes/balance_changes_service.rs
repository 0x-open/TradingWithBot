use std::{sync::Arc, time::Duration};

use futures::FutureExt;
use mockall_double::double;
use tokio::sync::mpsc;

#[double]
use crate::core::exchanges::general::currency_pair_to_metadata_converter::CurrencyPairToMetadataConverter;
#[double]
use crate::core::misc::time::time_manager;
#[double]
use crate::core::services::usd_converter::usd_converter::UsdConverter;

use crate::core::{
    balance_changes::balance_changes_accumulator::BalanceChangeAccumulator,
    infrastructure::spawn_by_timer,
    lifecycle::{application_manager::ApplicationManager, cancellation_token::CancellationToken},
    orders::{fill::OrderFill, order::ClientOrderFillId, pool::OrderRef},
    service_configuration::configuration_descriptor::ConfigurationDescriptor,
    DateTime,
};

use super::{
    balance_change_calculator_result::BalanceChangesCalculatorResult,
    balance_changes_calculator::BalanceChangesCalculator,
    profit_loss_balance_change::ProfitLossBalanceChange,
    profit_loss_stopper_service::ProfitLossStopperService,
};

enum BalanceChangeServiceEvent {
    OnTimer,
    BalanceChange(BalanceChange),
}

struct BalanceChange {
    pub balance_changes: BalanceChangesCalculatorResult,
    pub client_order_fill_id: ClientOrderFillId,
    pub change_date: DateTime,
}

impl BalanceChange {
    pub fn new(
        balance_changes: BalanceChangesCalculatorResult,
        client_order_fill_id: ClientOrderFillId,
        change_date: DateTime,
    ) -> Self {
        Self {
            balance_changes,
            client_order_fill_id,
            change_date,
        }
    }
}

pub struct BalanceChangesService {
    usd_converter: UsdConverter,
    // TODO: fix me when DatabaseManager/DataRecorder will be implemented
    // private readonly IDatabaseManager _databaseManager;
    // private readonly IDataRecorder _dataRecorder;
    rx_event: mpsc::Receiver<BalanceChangeServiceEvent>,
    tx_event: mpsc::Sender<BalanceChangeServiceEvent>,
    balance_changes_accumulators: Vec<Arc<dyn BalanceChangeAccumulator + Send + Sync>>,
    profit_loss_stopper_service: Arc<ProfitLossStopperService>,
    balance_changes_calculator: BalanceChangesCalculator,
    application_manager: Arc<ApplicationManager>,
}

impl BalanceChangesService {
    pub fn new(
        currency_pair_to_metadata_converter: Arc<CurrencyPairToMetadataConverter>,
        profit_loss_stopper_service: Arc<ProfitLossStopperService>,
        usd_converter: UsdConverter,
        application_manager: Arc<ApplicationManager>,
        // IDatabaseManager databaseManager,
        // IDataRecorder dataRecorder,
    ) -> Arc<Self> {
        let (tx_event, rx_event) = mpsc::channel(20_000);
        let balance_changes_accumulators =
            vec![profit_loss_stopper_service.clone()
                as Arc<dyn BalanceChangeAccumulator + Send + Sync>];

        let this = Arc::new(Self {
            usd_converter,
            // _databaseManager = databaseManager;
            // _dataRecorder = dataRecorder;
            rx_event,
            tx_event,
            balance_changes_accumulators,
            profit_loss_stopper_service,
            balance_changes_calculator: BalanceChangesCalculator::new(
                currency_pair_to_metadata_converter,
            ),
            application_manager: application_manager.clone(),
        });

        let on_timer_tick = {
            let this = this.clone();
            move || {
                let this = this.clone();
                let application_manager = application_manager.clone();
                async move {
                    if application_manager.stop_token().is_cancellation_requested() {
                        return;
                    }
                    let _ = this.tx_event.send(BalanceChangeServiceEvent::OnTimer).await.map_err(|_|
                        panic!(
                            "BalanceChangesService::timer_action: Unable to send event, probably receiver is dropped already"
                        )
                    );
                }.boxed()
            }
        };

        let _ = spawn_by_timer(
            on_timer_tick,
            "BalanceChangesService",
            Duration::ZERO,
            Duration::from_secs(5),
            true,
        );

        this
    }

    pub async fn run(&mut self, cancellation_token: CancellationToken) {
        // TODO: fix me when DatabaseManager/DataRecorder will be implemented
        //             if (_databaseManager != null)
        //             {
        //                 await Task.WhenAll(_balanceChangeAccumulators.Select(x => x.LoadData(_databaseManager, cancellationToken)));
        //                 await _profitLossStopperService.CheckForLimit(_usdConverter, cancellationToken);
        //             }

        loop {
            let new_event = tokio::select! {
                event = self.rx_event.recv() => event,
                _ = cancellation_token.when_cancelled() => return,
            }.expect("BalanceChangesService::run() the event channel is closed but cancellation hasn't been requested");

            match new_event {
                BalanceChangeServiceEvent::BalanceChange(event) => {
                    self.handle_balance_change_event(event, cancellation_token.clone())
                        .await;
                }
                BalanceChangeServiceEvent::OnTimer => {
                    self.profit_loss_stopper_service
                        .check_for_limit(&self.usd_converter, cancellation_token.clone())
                        .await;
                }
            }
        }
    }

    async fn handle_balance_change_event(
        &self,
        event: BalanceChange,
        cancellation_token: CancellationToken,
    ) {
        let changes = event.balance_changes.get_changes();
        for (request, balance_change) in changes.get_as_balances() {
            let usd_change = event
                .balance_changes
                .calculate_usd_change(
                    request.currency_code,
                    balance_change,
                    &self.usd_converter,
                    cancellation_token.clone(),
                )
                .await;

            let profit_loss_balance_change = ProfitLossBalanceChange::new(
                request,
                event.balance_changes.exchange_id,
                event.client_order_fill_id.clone(),
                event.change_date,
                balance_change,
                usd_change,
            );

            // TODO: fix me when DataRecorder will be added
            // _dataRecorder.Save(profitLossBalanceChange);

            for accumulator in self.balance_changes_accumulators.iter() {
                accumulator.add_balance_change(&profit_loss_balance_change);
            }
        }
        self.profit_loss_stopper_service
            .check_for_limit(&self.usd_converter, cancellation_token)
            .await;
    }

    pub async fn add_balance_change(
        &self,
        configuration_descriptor: Arc<ConfigurationDescriptor>,
        order: &OrderRef,
        order_fill: OrderFill,
    ) {
        if self
            .application_manager
            .stop_token()
            .is_cancellation_requested()
        {
            return;
        }

        let client_order_fill_id = order_fill
            .client_order_fill_id()
            .clone()
            .expect("client_order_fill_id is None");

        let balance_changes = self.balance_changes_calculator.get_balance_changes(
            configuration_descriptor,
            order,
            order_fill,
        );
        let balance_changes_event = BalanceChangeServiceEvent::BalanceChange(BalanceChange::new(
            balance_changes,
            client_order_fill_id,
            time_manager::now(),
        ));

        let _ = self.tx_event.send(balance_changes_event).await.map_err(|_|
            panic!("BalanceChangesService::add_balance_change: Unable to send event, probably receiver is dropped already")
        );
    }
}
