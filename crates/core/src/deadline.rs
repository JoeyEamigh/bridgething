use tokio::time::Instant;

pub async fn past_deadline(at: Option<Instant>) {
  match at {
    Some(at) => tokio::time::sleep_until(at).await,
    None => std::future::pending().await,
  }
}
