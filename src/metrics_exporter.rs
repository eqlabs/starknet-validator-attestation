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
    metrics::describe_gauge!(
        "validator_attestation_starknet_latest_block_number",
        metrics::Unit::Count,
        "Latest block number seen by the validator"
    );
}
