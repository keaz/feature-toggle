//! `fluxgate watch`: follow a live stream over the backend's WebSocket.

use clap::Args;
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::{App, optional_team, query_pairs};
use crate::error::CliError;
use crate::output::Outcome;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Stream {
    EvaluationSummary,
    EvaluationRates,
    EvaluationsByFeature,
    EvaluationDashboard,
    SystemMetrics,
    RecentActivities,
    FeatureGrowth,
    ApprovalRequests,
}

impl Stream {
    fn api_name(self) -> &'static str {
        match self {
            Stream::EvaluationSummary => "evaluation-summary",
            Stream::EvaluationRates => "evaluation-rates",
            Stream::EvaluationsByFeature => "evaluations-by-feature",
            Stream::EvaluationDashboard => "evaluation-dashboard",
            Stream::SystemMetrics => "system-metrics",
            Stream::RecentActivities => "recent-activities",
            Stream::FeatureGrowth => "feature-growth",
            Stream::ApprovalRequests => "approval-requests",
        }
    }
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    #[arg(value_enum)]
    pub stream: Stream,
    /// Stop after this many messages.
    #[arg(long)]
    pub count: Option<u64>,
    /// Query parameter such as featureKey=checkout; repeat for several. The
    /// team is added as teamId when one is configured.
    #[arg(long = "query", value_name = "KEY=VALUE")]
    pub query: Vec<String>,
}

pub async fn run(args: WatchArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let extra = query_pairs(&args.query)?;
    let context = app.connect().await?;
    let mut url = context.api.url(&["ws"]);
    let scheme = match url.scheme() {
        "https" => "wss",
        _ => "ws",
    };
    url.set_scheme(scheme)
        .map_err(|_| CliError::Usage(format!("cannot stream from {url}")))?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("stream", args.stream.api_name());
        if let Some(team) = optional_team(&context).await? {
            pairs.append_pair("teamId", &team);
        }
        for (key, value) in &extra {
            pairs.append_pair(key, value);
        }
    }
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|err| CliError::Usage(format!("cannot stream from {url}: {err}")))?;
    if let Some(token) = context.api.token() {
        let value = format!("Bearer {token}")
            .parse()
            .map_err(|_| CliError::Usage("the token is not a valid header value".into()))?;
        request.headers_mut().insert("authorization", value);
    }
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|err| CliError::Network(format!("cannot open the stream: {err}")))?;

    let mut received = 0u64;
    while args.count.is_none_or(|count| received < count) {
        let Some(message) = socket.next().await else {
            break;
        };
        let text =
            match message.map_err(|err| CliError::Network(format!("stream failed: {err}")))? {
                Message::Text(text) => text.to_string(),
                Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Message::Close(_) => break,
                _ => continue,
            };
        // One compact JSON document per line, so the output can be piped.
        let line = serde_json::from_str::<serde_json::Value>(&text)
            .map(|value| value.to_string())
            .unwrap_or(text);
        writeln!(app.out, "{line}")?;
        app.out.flush()?;
        received += 1;
    }
    let _ = socket.close(None).await;
    Ok(Outcome::raw(String::new()))
}
