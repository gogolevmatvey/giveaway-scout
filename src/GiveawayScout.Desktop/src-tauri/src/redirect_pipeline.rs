//! Native test fixtures replace only transports; the runtime is shared with CLI.
#[cfg(feature = "ui-tests")]
struct UiRedirectTransport;

#[cfg(feature = "ui-tests")]
impl giveaway_scout_core::RedirectTransport for UiRedirectTransport {
    async fn get_redirect(
        &self,
        url: &url::Url,
    ) -> Result<giveaway_scout_core::RedirectResponse, giveaway_scout_core::OriginResolveError>
    {
        crate::ui_tests::resolve_redirect(url.as_str())
            .await
            .map(|location| giveaway_scout_core::RedirectResponse {
                status: 302,
                location: Some(location),
            })
            .map_err(giveaway_scout_core::OriginResolveError)
    }
}

pub(crate) fn dependencies(
    sink: giveaway_scout_runtime::NotificationSink,
    observer: giveaway_scout_runtime::RuntimeObserver,
) -> giveaway_scout_runtime::RuntimeDependencies {
    #[allow(unused_mut)]
    let mut dependencies = giveaway_scout_runtime::RuntimeDependencies::production(sink, observer);
    #[cfg(feature = "ui-tests")]
    {
        dependencies.fetcher_factory = std::sync::Arc::new(|settings| {
            crate::ui_tests::UiPageFetcher::with_policy(
                settings.http_timeout_seconds,
                settings.http_retry_count,
            )
            .map(giveaway_scout_runtime::source_fetcher)
            .map_err(|error| error.to_string())
        });
        dependencies.resolver =
            giveaway_scout_runtime::resolver_from_transport(UiRedirectTransport);
    }
    dependencies
}
