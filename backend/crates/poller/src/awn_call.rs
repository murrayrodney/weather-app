//! `awn_call`: the only place the poller reserves rate gates and the only place it retries (03 section 5.3).
//!
//! Every AWN request first reserves `awn:key:<credential_id>` and then `awn:app:poller` (key gate, then app gate),
//! sleeps the database-computed waits, and makes one request. Reservations are made in this order everywhere, so
//! they cannot deadlock. Gates apply in every `AWN_MODE`, replay included.

use std::time::Duration;

use famweather_core::awn::AwnError;
use famweather_core::ratelimit::{reserve, GateBusy};
use futures::future::BoxFuture;
use uuid::Uuid;

use crate::env::Env;

/// Attempts in all when AWN keeps answering 429.
pub const RATE_LIMIT_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallError {
    #[error("AWN rejected the API key")]
    Unauthorized,
    #[error("AWN rate limit reached")]
    RateLimited,
    #[error("AWN unavailable")]
    Unavailable,
    #[error("AWN returned an unusable response")]
    BadResponse,
    /// A gate could not be reserved: counts as a transient unit failure.
    #[error("{0}")]
    Busy(GateBusy),
    /// Shutdown was requested while waiting.
    #[error("interrupted")]
    Interrupted,
}

/// Sleeps `d`, returning `false` as soon as shutdown is requested.
pub(crate) async fn sleep_or_cancel(env: &Env, d: Duration) -> bool {
    if d.is_zero() {
        return !env.cancel.is_cancelled();
    }
    tokio::select! {
        () = tokio::time::sleep(d) => true,
        () = env.cancel.cancelled() => false,
    }
}

fn jittered(env: &Env, d: Duration) -> Duration {
    if !env.backoff.jitter {
        return d;
    }
    // +-20% from the sub-second clock: no randomness crate needed for spreading retries.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.subsec_nanos());
    let factor = 0.8 + 0.4 * (f64::from(nanos % 1000) / 1000.0);
    d.mul_f64(factor)
}

async fn gate(env: &Env, name: &str, spacing: Duration) -> Result<(), CallError> {
    let wait = reserve(&env.pool, name, spacing, env.gates.max_wait)
        .await
        .map_err(CallError::Busy)?;
    if sleep_or_cancel(env, wait).await {
        Ok(())
    } else {
        Err(CallError::Interrupted)
    }
}

/// One logical AWN call for `credential_id`, with gates and the documented retries:
/// 429 up to five attempts (`Retry-After` or 2, 4, 8, 16 s), 5xx/undecodable up to three retries (1, 3, 9 s),
/// 401/403 immediately.
pub async fn awn_call<'a, T, F>(env: &Env, credential_id: Uuid, op: F) -> Result<T, CallError>
where
    F: Fn() -> BoxFuture<'a, Result<T, AwnError>>,
{
    let key_gate = format!("awn:key:{credential_id}");
    let mut limited = 0u32;
    let mut unavailable = 0usize;
    loop {
        if env.cancel.is_cancelled() {
            return Err(CallError::Interrupted);
        }
        gate(env, &key_gate, env.gates.key_spacing).await?;
        gate(env, "awn:app:poller", env.gates.app_spacing).await?;
        let err = match op().await {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        match err {
            AwnError::Unauthorized => return Err(CallError::Unauthorized),
            AwnError::RateLimited { retry_after } => {
                limited += 1;
                if limited >= RATE_LIMIT_ATTEMPTS {
                    return Err(CallError::RateLimited);
                }
                let wait = match retry_after {
                    Some(d) => d.min(env.backoff.max_retry_after),
                    None => jittered(
                        env,
                        env.backoff
                            .rate_limited
                            .get(limited as usize - 1)
                            .or(env.backoff.rate_limited.last())
                            .copied()
                            .unwrap_or_default(),
                    ),
                };
                if !sleep_or_cancel(env, wait).await {
                    return Err(CallError::Interrupted);
                }
            }
            e @ (AwnError::Unavailable | AwnError::BadResponse) => {
                if unavailable >= env.backoff.unavailable.len() {
                    return Err(if e == AwnError::Unavailable {
                        CallError::Unavailable
                    } else {
                        CallError::BadResponse
                    });
                }
                let wait = env.backoff.unavailable[unavailable];
                unavailable += 1;
                if !sleep_or_cancel(env, wait).await {
                    return Err(CallError::Interrupted);
                }
            }
        }
    }
}
