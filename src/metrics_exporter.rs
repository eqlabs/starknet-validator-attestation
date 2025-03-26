use metrics_exporter_prometheus::PrometheusHandle;

#[derive(Clone)]
struct State {
    prometheus_handle: PrometheusHandle,
}

pub async fn spawn(
    addr: impl Into<std::net::SocketAddr> + 'static,
    prometheus_handle: PrometheusHandle,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    describe_metrics();

    let app = axum::Router::new()
        .route("/metrics", axum::routing::get(metrics_route))
        .with_state(State { prometheus_handle });
    let listener = tokio::net::TcpListener::bind(addr.into()).await?;
    let handle = tokio::task::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("server error")
    });

    Ok(handle)
}

async fn metrics_route(axum::extract::State(state): axum::extract::State<State>) -> String {
    state.prometheus_handle.render()
}

fn describe_metrics() {
    // Starknet
    metrics::describe_gauge!(
        "validator_attestation_starknet_latest_block_number",
        metrics::Unit::Count,
        "Latest block number seen by the validator"
    );

    // Epoch
    metrics::describe_gauge!(
        "validator_attestation_current_epoch_id",
        metrics::Unit::Count,
        "Current epoch ID"
    );
    metrics::describe_gauge!(
        "validator_attestation_current_epoch_starting_block_number",
        metrics::Unit::Count,
        "Current epoch starting block number"
    );
    metrics::describe_gauge!(
        "validator_attestation_current_epoch_length",
        metrics::Unit::Count,
        "Current epoch length"
    );
    metrics::describe_gauge!(
        "validator_attestation_current_epoch_assigned_block_number",
        metrics::Unit::Count,
        "Block number to attest in current epoch"
    );

    // Attestation
    metrics::describe_gauge!(
        "validator_attestation_last_attestation_timestamp_seconds",
        metrics::Unit::Seconds,
        "Timestamp of the last attestation"
    );
}
