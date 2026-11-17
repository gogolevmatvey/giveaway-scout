use std::{collections::HashSet, future::Future, sync::Arc, time::Duration};

use futures_util::{StreamExt, stream};
use tokio::sync::Semaphore;
use url::{Host, Url};

use crate::{is_free_steam_keys_redirect, known_destination};

const RESOLUTION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct OriginResolveError(pub String);

#[derive(Debug, Clone)]
pub struct RedirectResponse {
    pub status: u16,
    pub location: Option<String>,
}

pub trait RedirectTransport: Send + Sync {
    fn get_redirect(
        &self,
        url: &Url,
    ) -> impl Future<Output = Result<RedirectResponse, OriginResolveError>> + Send;
}

pub struct HttpRedirectTransport {
    client: reqwest::Client,
}

impl HttpRedirectTransport {
    pub fn new() -> Result<Self, OriginResolveError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(RESOLUTION_TIMEOUT)
            .timeout(RESOLUTION_TIMEOUT)
            .user_agent(concat!("GiveawayScout/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| OriginResolveError(error.to_string()))?;
        Ok(Self { client })
    }
}

impl RedirectTransport for HttpRedirectTransport {
    async fn get_redirect(&self, url: &Url) -> Result<RedirectResponse, OriginResolveError> {
        // Only a known FreeSteamKeys URL is fetched. The destination body is
        // never requested: a protected Gleam page is not needed for identity.
        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(|error| OriginResolveError(error.to_string()))?;
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|error| OriginResolveError(error.to_string()))?;
        Ok(RedirectResponse {
            status: response.status().as_u16(),
            location,
        })
    }
}

pub struct OriginResolver<T = HttpRedirectTransport> {
    transport: T,
    permits: Arc<Semaphore>,
}

impl OriginResolver<HttpRedirectTransport> {
    pub fn new() -> Result<Self, OriginResolveError> {
        Ok(Self::with_transport(HttpRedirectTransport::new()?))
    }
}

impl<T: RedirectTransport> OriginResolver<T> {
    pub fn with_transport(transport: T) -> Self {
        Self {
            transport,
            permits: Arc::new(Semaphore::new(2)),
        }
    }

    pub async fn resolve(&self, original: &str) -> Result<String, OriginResolveError> {
        if !is_free_steam_keys_redirect(original) {
            return Err(OriginResolveError("unsupported redirect source".into()));
        }
        tokio::time::timeout(RESOLUTION_TIMEOUT, self.resolve_inner(original))
            .await
            .map_err(|_| OriginResolveError("origin lookup timed out".into()))?
    }

    async fn resolve_inner(&self, original: &str) -> Result<String, OriginResolveError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| OriginResolveError("origin resolver is unavailable".into()))?;
        let mut current =
            Url::parse(original).map_err(|error| OriginResolveError(error.to_string()))?;
        let mut visited = HashSet::new();
        for _ in 0..5 {
            if !visited.insert(current.to_string()) {
                return Err(OriginResolveError("redirect loop".into()));
            }
            let response = self.transport.get_redirect(&current).await?;
            if !matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                return Err(OriginResolveError(format!(
                    "origin redirect unavailable: HTTP {}",
                    response.status
                )));
            }
            let location = response
                .location
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| OriginResolveError("redirect has no Location header".into()))?;
            let target = current
                .join(location.trim())
                .map_err(|error| OriginResolveError(error.to_string()))?;
            if !safe_destination(&target) {
                return Err(OriginResolveError(
                    "unsupported redirect destination".into(),
                ));
            }
            if let Some(identity) = known_destination(target.as_str()) {
                return Ok(identity.url);
            }
            if !matches!(
                target.host_str(),
                Some("www.freesteamkeys.com" | "freesteamkeys.com")
            ) {
                // Keep an ordinary external destination as an opening URL, but
                // do not infer equality from generic product or landing pages.
                return Ok(target.to_string());
            }
            if target.scheme() != "https" {
                return Err(OriginResolveError("insecure internal redirect".into()));
            }
            current = target;
        }
        Err(OriginResolveError("too many redirects".into()))
    }

    /// One time budget covers the complete batch, including waiting for the
    /// two global resolver permits. Unfinished items retain their input URLs.
    pub async fn resolve_batch(
        &self,
        urls: Vec<String>,
    ) -> Vec<(String, Result<String, OriginResolveError>)> {
        let mut pending: Vec<Option<String>> = urls.iter().cloned().map(Some).collect();
        let work = stream::iter(urls.into_iter().enumerate().map(|(index, url)| async move {
            let result = self.resolve(&url).await;
            (index, url, result)
        }))
        .buffer_unordered(2);
        tokio::pin!(work);
        let deadline = tokio::time::sleep(RESOLUTION_TIMEOUT);
        tokio::pin!(deadline);
        let mut results = Vec::new();
        loop {
            tokio::select! {
                biased;
                () = &mut deadline => break,
                result = work.next() => match result {
                    Some((index, url, result)) => {
                        pending[index] = None;
                        results.push((url, result));
                    }
                    None => break,
                },
            }
        }
        results.extend(pending.into_iter().flatten().map(|url| {
            (
                url,
                Err(OriginResolveError(
                    "origin batch time budget exhausted".into(),
                )),
            )
        }));
        results
    }
}

fn safe_destination(url: &Url) -> bool {
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return false;
    }
    match url.host() {
        Some(Host::Domain(name)) => {
            name.contains('.')
                && name != "localhost"
                && !name.ends_with(".localhost")
                && !name.ends_with(".local")
        }
        // Never browse or promote addresses of local services from untrusted
        // remote headers. Actual requests are restricted to FreeSteamKeys.
        _ => false,
    }
}
