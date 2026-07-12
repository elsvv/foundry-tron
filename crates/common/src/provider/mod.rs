//! Provider-related instantiation and usage utilities.

pub mod curl_transport;
pub mod fee;
pub mod mpp;
pub mod runtime_transport;

use crate::{
    ALCHEMY_FREE_TIER_CUPS, REQUEST_TIMEOUT,
    provider::{curl_transport::CurlTransport, runtime_transport::RuntimeTransportBuilder},
};
use alloy_chains::NamedChain;
use alloy_json_rpc::{Id, RequestPacket, Response, ResponsePacket, ResponsePayload};
use alloy_network::{Network, NetworkWallet};
use alloy_provider::{
    Identity, ProviderBuilder as AlloyProviderBuilder, RootProvider,
    fillers::{FillProvider, JoinFill, RecommendedFillers, WalletFiller},
    network::{AnyNetwork, EthereumWallet},
};
use alloy_rpc_client::ClientBuilder;
use alloy_transport::{
    TransportError, TransportFut, layers::RetryBackoffLayer, utils::guess_local_url,
};
use eyre::{Result, WrapErr};
use foundry_config::Config;
use reqwest::Url;
use serde_json::value::to_raw_value;
use std::{
    marker::PhantomData,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tower::{Layer, Service};
use url::ParseError;

/// The assumed block time for unknown chains.
/// We assume that these are chains have a faster block time.
const DEFAULT_UNKNOWN_CHAIN_BLOCK_TIME: Duration = Duration::from_secs(3);

/// The factor to scale the block time by to get the poll interval.
const POLL_INTERVAL_BLOCK_TIME_SCALE_FACTOR: f32 = 0.6;

/// Helper type alias for a retry provider
pub type RetryProvider<N = AnyNetwork> = RootProvider<N>;

/// Helper type alias for a retry provider with a signer
pub type RetryProviderWithSigner<N = AnyNetwork, W = EthereumWallet> = FillProvider<
    JoinFill<JoinFill<Identity, <N as RecommendedFillers>::RecommendedFillers>, WalletFiller<W>>,
    RootProvider<N>,
    N,
>;

/// Constructs a provider with a 100 millisecond interval poll if it's a localhost URL (most likely
/// an anvil or other dev node) and with the default, or 7 second otherwise.
///
/// See [`try_get_http_provider`] for more details.
///
/// # Panics
///
/// Panics if the URL is invalid.
///
/// # Examples
///
/// ```
/// use foundry_common::provider::get_http_provider;
///
/// let retry_provider = get_http_provider("http://localhost:8545");
/// ```
#[inline]
#[track_caller]
pub fn get_http_provider(builder: impl AsRef<str>) -> RetryProvider {
    try_get_http_provider(builder).unwrap()
}

/// Constructs a provider with a 100 millisecond interval poll if it's a localhost URL (most likely
/// an anvil or other dev node) and with the default, or 7 second otherwise.
#[inline]
pub fn try_get_http_provider(builder: impl AsRef<str>) -> Result<RetryProvider> {
    ProviderBuilder::new(builder.as_ref()).build()
}

/// A round-robin transport that distributes requests across multiple transports.
///
/// Each request is sent to exactly one transport, rotating through the list.
/// Failover on error is handled by the retry layer above this service.
#[derive(Clone)]
pub struct RoundRobinService<S> {
    transports: Arc<Vec<S>>,
    next: Arc<AtomicUsize>,
}

impl<S> RoundRobinService<S> {
    /// Creates a new round-robin service from a non-empty list of transports.
    ///
    /// # Panics
    ///
    /// Panics if `transports` is empty.
    pub fn new(transports: Vec<S>) -> Self {
        assert!(!transports.is_empty(), "RoundRobinService requires at least one transport");
        Self { transports: Arc::new(transports), next: Arc::new(AtomicUsize::new(0)) }
    }
}

impl<S> Service<RequestPacket> for RoundRobinService<S>
where
    S: Service<
            RequestPacket,
            Response = ResponsePacket,
            Error = TransportError,
            Future = TransportFut<'static>,
        > + Clone
        + Send
        + Sync
        + 'static,
{
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: RequestPacket) -> Self::Future {
        let transports = self.transports.clone();
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % transports.len();
        let mut transport = transports[idx].clone();
        transport.call(req)
    }
}

/// The JSON-RPC method that a Tron `/jsonrpc` node intentionally answers with a permanent
/// `-32601` "method not found" stub.
const ETH_GET_TRANSACTION_COUNT: &str = "eth_getTransactionCount";

/// A [`tower::Layer`] that short-circuits `eth_getTransactionCount` requests with a synthetic
/// `"0x0"` response before they reach the network.
///
/// java-tron's `/jsonrpc` endpoint serves everything the fork backend needs (balance, code,
/// storage, blocks, chain id) but deliberately returns a permanent `-32601` for
/// `eth_getTransactionCount` (there is no EVM CREATE-nonce on Tron — deploy addresses are
/// txid-derived). foundry-fork-db loads accounts via `try_join3(balance, nonce, code)`, so that
/// single `-32601` aborts every account fetch and makes a read-only fork non-functional. This
/// layer answers `eth_getTransactionCount` locally with `0x0`, which is correct for Tron and
/// harmless for read-only forks (foundry increments its own in-memory nonce from this base).
///
/// It is applied strictly on the Tron fork path (gated by [`NetworkConfigs::is_tron`] at the
/// provider-construction seam); every other method passes through untouched.
#[derive(Clone, Copy, Debug, Default)]
pub struct TronNonceShimLayer;

impl<S> Layer<S> for TronNonceShimLayer {
    type Service = TronNonceShimService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TronNonceShimService { inner }
    }
}

/// The [`tower::Service`] produced by [`TronNonceShimLayer`].
#[derive(Clone, Debug)]
pub struct TronNonceShimService<S> {
    inner: S,
}

/// Builds a synthetic successful `eth_getTransactionCount` response of `0x0` for the given request
/// [`Id`], without touching the network.
fn tron_zero_nonce_response(id: Id) -> Response {
    // Serializing a static hex quantity string can never fail.
    let result = to_raw_value(&"0x0").expect("serializing a static hex string cannot fail");
    Response { id, payload: ResponsePayload::Success(result) }
}

impl<S> Service<RequestPacket> for TronNonceShimService<S>
where
    S: Service<
            RequestPacket,
            Response = ResponsePacket,
            Error = TransportError,
            Future = TransportFut<'static>,
        > + Clone
        + Send
        + 'static,
{
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: RequestPacket) -> Self::Future {
        match &req {
            // Fast path: a lone `eth_getTransactionCount` is answered locally with `0x0`.
            RequestPacket::Single(single) if single.method() == ETH_GET_TRANSACTION_COUNT => {
                let response = tron_zero_nonce_response(single.id().clone());
                Box::pin(async move { Ok(ResponsePacket::Single(response)) })
            }
            // A batch made up entirely of `eth_getTransactionCount` is answered locally too.
            RequestPacket::Batch(reqs)
                if !reqs.is_empty()
                    && reqs.iter().all(|r| r.method() == ETH_GET_TRANSACTION_COUNT) =>
            {
                let responses =
                    reqs.iter().map(|r| tron_zero_nonce_response(r.id().clone())).collect();
                Box::pin(async move { Ok(ResponsePacket::Batch(responses)) })
            }
            // Everything else (including mixed batches) passes through untouched.
            _ => {
                let mut inner = self.inner.clone();
                inner.call(req)
            }
        }
    }
}

/// Helper type to construct a `RetryProvider`
///
/// This builder is generic over the network type `N`, defaulting to `AnyNetwork`.
#[derive(Debug)]
pub struct ProviderBuilder<N: Network = AnyNetwork> {
    // Note: this is a result, so we can easily chain builder calls
    url: Result<Url>,
    chain: NamedChain,
    max_retry: u32,
    initial_backoff: u64,
    timeout: Duration,
    /// available CUPS
    compute_units_per_second: u64,
    /// JWT Secret
    jwt: Option<String>,
    headers: Vec<String>,
    is_local: bool,
    /// Whether to accept invalid certificates.
    accept_invalid_certs: bool,
    /// Whether to disable automatic proxy detection.
    no_proxy: bool,
    /// Whether to output curl commands instead of making requests.
    curl_mode: bool,
    /// Whether to install the Tron nonce shim on the RPC client.
    ///
    /// When enabled, `eth_getTransactionCount` is answered locally with `0x0` instead of hitting
    /// the network. This is required to fork a java-tron `/jsonrpc` node, which serves a permanent
    /// `-32601` for that method. See [`TronNonceShimLayer`].
    tron_shim: bool,
    /// Phantom data for the network type.
    _network: PhantomData<N>,
}

impl<N: Network> ProviderBuilder<N> {
    /// Creates a new ProviderBuilder helper instance.
    pub fn new(url_str: &str) -> Self {
        // a copy is needed for the next lines to work
        let mut url_str = url_str;

        // invalid url: non-prefixed URL scheme is not allowed, so we prepend the default http
        // prefix
        let storage;
        if url_str.starts_with("localhost:") {
            storage = format!("http://{url_str}");
            url_str = storage.as_str();
        }

        let url = Url::parse(url_str)
            .or_else(|err| match err {
                ParseError::RelativeUrlWithoutBase => {
                    if SocketAddr::from_str(url_str).is_ok() {
                        Url::parse(&format!("http://{url_str}"))
                    } else {
                        let path = Path::new(url_str);

                        if let Ok(path) = resolve_path(path) {
                            Url::parse(&format!("file://{}", path.display()))
                        } else {
                            Err(err)
                        }
                    }
                }
                _ => Err(err),
            })
            .wrap_err_with(|| format!("invalid provider URL: {url_str:?}"));

        // Use the final URL string to guess if it's a local URL.
        let is_local = url.as_ref().is_ok_and(|url| guess_local_url(url.as_str()));

        Self {
            url,
            chain: NamedChain::Mainnet,
            max_retry: 8,
            initial_backoff: 800,
            timeout: REQUEST_TIMEOUT,
            // alchemy max cpus <https://docs.alchemy.com/reference/compute-units#what-are-cups-compute-units-per-second>
            compute_units_per_second: ALCHEMY_FREE_TIER_CUPS,
            jwt: None,
            headers: vec![],
            is_local,
            accept_invalid_certs: false,
            no_proxy: false,
            curl_mode: false,
            tron_shim: false,
            _network: PhantomData,
        }
    }

    /// Constructs a [ProviderBuilder] instantiated using [Config] values.
    ///
    /// Defaults to `http://localhost:8545` and `Mainnet`.
    pub fn from_config(config: &Config) -> Result<Self> {
        let url = config.get_rpc_url_or_localhost_http()?;
        let mut builder = Self::new(url.as_ref())
            .accept_invalid_certs(config.eth_rpc_accept_invalid_certs)
            .no_proxy(config.eth_rpc_no_proxy)
            .curl_mode(config.eth_rpc_curl);

        if let Ok(chain) = config.chain.unwrap_or_default().try_into() {
            builder = builder.chain(chain);
        }

        if let Some(jwt) = config.get_rpc_jwt_secret()? {
            builder = builder.jwt(jwt.as_ref());
        }

        if let Some(rpc_timeout) = config.eth_rpc_timeout {
            builder = builder.timeout(Duration::from_secs(rpc_timeout));
        }

        if let Some(rpc_headers) = config.eth_rpc_headers.clone() {
            builder = builder.headers(rpc_headers);
        }

        Ok(builder)
    }

    /// Enables a request timeout.
    ///
    /// The timeout is applied from when the request starts connecting until the
    /// response body has finished.
    ///
    /// Default is no timeout.
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the chain of the node the provider will connect to
    pub const fn chain(mut self, chain: NamedChain) -> Self {
        self.chain = chain;
        self
    }

    /// How often to retry a failed request
    pub const fn max_retry(mut self, max_retry: u32) -> Self {
        self.max_retry = max_retry;
        self
    }

    /// How often to retry a failed request. If `None`, defaults to the already-set value.
    pub fn maybe_max_retry(mut self, max_retry: Option<u32>) -> Self {
        self.max_retry = max_retry.unwrap_or(self.max_retry);
        self
    }

    /// The starting backoff delay to use after the first failed request. If `None`, defaults to
    /// the already-set value.
    pub fn maybe_initial_backoff(mut self, initial_backoff: Option<u64>) -> Self {
        self.initial_backoff = initial_backoff.unwrap_or(self.initial_backoff);
        self
    }

    /// The starting backoff delay to use after the first failed request
    pub const fn initial_backoff(mut self, initial_backoff: u64) -> Self {
        self.initial_backoff = initial_backoff;
        self
    }

    /// Sets the number of assumed available compute units per second
    ///
    /// See also, <https://docs.alchemy.com/reference/compute-units#what-are-cups-compute-units-per-second>
    pub const fn compute_units_per_second(mut self, compute_units_per_second: u64) -> Self {
        self.compute_units_per_second = compute_units_per_second;
        self
    }

    /// Sets the number of assumed available compute units per second
    ///
    /// See also, <https://docs.alchemy.com/reference/compute-units#what-are-cups-compute-units-per-second>
    pub const fn compute_units_per_second_opt(
        mut self,
        compute_units_per_second: Option<u64>,
    ) -> Self {
        if let Some(cups) = compute_units_per_second {
            self.compute_units_per_second = cups;
        }
        self
    }

    /// Sets the provider to be local.
    ///
    /// This is useful for local dev nodes.
    pub const fn local(mut self, is_local: bool) -> Self {
        self.is_local = is_local;
        self
    }

    /// Sets aggressive `max_retry` and `initial_backoff` values
    ///
    /// This is only recommend for local dev nodes
    pub const fn aggressive(self) -> Self {
        self.max_retry(100).initial_backoff(100).local(true)
    }

    /// Sets the JWT secret
    pub fn jwt(mut self, jwt: impl Into<String>) -> Self {
        self.jwt = Some(jwt.into());
        self
    }

    /// Sets http headers
    pub fn headers(mut self, headers: Vec<String>) -> Self {
        self.headers = headers;

        self
    }

    /// Sets http headers. If `None`, defaults to the already-set value.
    pub fn maybe_headers(mut self, headers: Option<Vec<String>>) -> Self {
        self.headers = headers.unwrap_or(self.headers);
        self
    }

    /// Sets whether to accept invalid certificates.
    pub const fn accept_invalid_certs(mut self, accept_invalid_certs: bool) -> Self {
        self.accept_invalid_certs = accept_invalid_certs;
        self
    }

    /// Sets whether to disable automatic proxy detection.
    ///
    /// This can help in sandboxed environments (e.g., Cursor IDE sandbox, macOS App Sandbox)
    /// where system proxy detection via SCDynamicStore causes crashes.
    pub const fn no_proxy(mut self, no_proxy: bool) -> Self {
        self.no_proxy = no_proxy;
        self
    }

    /// Sets whether to output curl commands instead of making requests.
    ///
    /// When enabled, the provider will print equivalent curl commands to stdout
    /// instead of actually executing the RPC requests.
    pub const fn curl_mode(mut self, curl_mode: bool) -> Self {
        self.curl_mode = curl_mode;
        self
    }

    /// Sets whether to install the Tron nonce shim on the RPC client.
    ///
    /// This must be enabled only when forking a Tron `/jsonrpc` node (gated by
    /// [`NetworkConfigs::is_tron`] at the caller). See [`TronNonceShimLayer`].
    pub const fn tron_shim(mut self, tron_shim: bool) -> Self {
        self.tron_shim = tron_shim;
        self
    }

    /// Constructs the `RetryProvider` taking all configs into account.
    pub fn build(self) -> Result<RetryProvider<N>> {
        let Self {
            url,
            chain,
            max_retry,
            initial_backoff,
            timeout,
            compute_units_per_second,
            jwt,
            headers,
            is_local,
            accept_invalid_certs,
            no_proxy,
            curl_mode,
            tron_shim,
            ..
        } = self;
        let url = url?;
        let no_proxy = no_proxy || is_local;

        let retry_layer =
            RetryBackoffLayer::new(max_retry, initial_backoff, compute_units_per_second);

        // If curl_mode is enabled, use CurlTransport instead of RuntimeTransport
        if curl_mode {
            let transport = CurlTransport::new(url).with_headers(headers).with_jwt(jwt);
            // On the Tron fork path, the shim is the outermost layer so `eth_getTransactionCount`
            // is answered locally before hitting retry/transport; every other method is unaffected.
            let client = if tron_shim {
                ClientBuilder::default()
                    .layer(TronNonceShimLayer)
                    .layer(retry_layer)
                    .transport(transport, is_local)
            } else {
                ClientBuilder::default().layer(retry_layer).transport(transport, is_local)
            };

            let provider = AlloyProviderBuilder::<_, _, N>::default()
                .connect_provider(RootProvider::new(client));

            return Ok(provider);
        }

        let transport = RuntimeTransportBuilder::new(url)
            .with_timeout(timeout)
            .with_headers(headers)
            .with_jwt(jwt)
            .accept_invalid_certs(accept_invalid_certs)
            .no_proxy(no_proxy)
            .build();
        let client = if tron_shim {
            ClientBuilder::default()
                .layer(TronNonceShimLayer)
                .layer(retry_layer)
                .transport(transport, is_local)
        } else {
            ClientBuilder::default().layer(retry_layer).transport(transport, is_local)
        };

        if !is_local {
            client.set_poll_interval(
                chain
                    .average_blocktime_hint()
                    // we cap the poll interval because if not provided, chain would default to
                    // mainnet
                    .map(|hint| hint.min(DEFAULT_UNKNOWN_CHAIN_BLOCK_TIME))
                    .unwrap_or(DEFAULT_UNKNOWN_CHAIN_BLOCK_TIME)
                    .mul_f32(POLL_INTERVAL_BLOCK_TIME_SCALE_FACTOR),
            );
        }

        let provider =
            AlloyProviderBuilder::<_, _, N>::default().connect_provider(RootProvider::new(client));

        Ok(provider)
    }
}

impl<N: Network> ProviderBuilder<N> {
    /// Constructs a `RetryProvider` backed by multiple URLs using round-robin load balancing.
    ///
    /// Each request is sent to exactly one transport, rotating through the list via
    /// [`RoundRobinService`]. There is no health scoring or endpoint deprioritization.
    /// On failure, the `RetryBackoffLayer` retries the request, which naturally hits
    /// the next transport in the rotation.
    pub fn build_fallback(self, urls: Vec<String>) -> Result<RetryProvider<N>> {
        let Self {
            chain,
            max_retry,
            initial_backoff,
            timeout,
            compute_units_per_second,
            jwt,
            headers,
            accept_invalid_certs,
            no_proxy,
            curl_mode,
            ..
        } = self;

        eyre::ensure!(!urls.is_empty(), "at least one fork URL is required");
        eyre::ensure!(!curl_mode, "curl mode is not supported with multiple fork URLs");

        // Build a RuntimeTransport for each URL, using the same URL normalization
        // as ProviderBuilder::new() (handles localhost:port, raw socket addrs, IPC paths)
        let mut parsed_urls = Vec::with_capacity(urls.len());
        let transports: Vec<_> = urls
            .iter()
            .map(|url_str| {
                let builder = Self::new(url_str);
                let url = builder.url?;
                let transport_no_proxy = no_proxy || builder.is_local;
                parsed_urls.push(url.clone());
                Ok(RuntimeTransportBuilder::new(url)
                    .with_timeout(timeout)
                    .with_headers(headers.clone())
                    .with_jwt(jwt.clone())
                    .accept_invalid_certs(accept_invalid_certs)
                    .no_proxy(transport_no_proxy)
                    .build())
            })
            .collect::<Result<Vec<_>>>()?;

        let round_robin = RoundRobinService::new(transports);

        let retry_layer =
            RetryBackoffLayer::new(max_retry, initial_backoff, compute_units_per_second);
        // Use normalized/parsed URLs for local detection, consistent with build()
        let is_local = parsed_urls.iter().all(|url| guess_local_url(url.as_str()));
        let client = ClientBuilder::default().layer(retry_layer).transport(round_robin, is_local);

        if !is_local {
            client.set_poll_interval(
                chain
                    .average_blocktime_hint()
                    .map(|hint| hint.min(DEFAULT_UNKNOWN_CHAIN_BLOCK_TIME))
                    .unwrap_or(DEFAULT_UNKNOWN_CHAIN_BLOCK_TIME)
                    .mul_f32(POLL_INTERVAL_BLOCK_TIME_SCALE_FACTOR),
            );
        }

        let provider =
            AlloyProviderBuilder::<_, _, N>::default().connect_provider(RootProvider::new(client));

        Ok(provider)
    }

    /// Constructs the `RetryProvider` with a wallet.
    pub fn build_with_wallet<W: NetworkWallet<N> + Clone>(
        self,
        wallet: W,
    ) -> Result<RetryProviderWithSigner<N, W>>
    where
        N: RecommendedFillers,
    {
        let Self {
            url,
            chain,
            max_retry,
            initial_backoff,
            timeout,
            compute_units_per_second,
            jwt,
            headers,
            is_local,
            accept_invalid_certs,
            no_proxy,
            curl_mode,
            ..
        } = self;
        let url = url?;
        let no_proxy = no_proxy || is_local;

        let retry_layer =
            RetryBackoffLayer::new(max_retry, initial_backoff, compute_units_per_second);

        // If curl_mode is enabled, use CurlTransport instead of RuntimeTransport
        if curl_mode {
            let transport = CurlTransport::new(url).with_headers(headers).with_jwt(jwt);
            let client = ClientBuilder::default().layer(retry_layer).transport(transport, is_local);

            let provider = AlloyProviderBuilder::<_, _, N>::default()
                .with_recommended_fillers()
                .wallet(wallet)
                .connect_provider(RootProvider::new(client));

            return Ok(provider);
        }

        let transport = RuntimeTransportBuilder::new(url)
            .with_timeout(timeout)
            .with_headers(headers)
            .with_jwt(jwt)
            .accept_invalid_certs(accept_invalid_certs)
            .no_proxy(no_proxy)
            .build();

        let client = ClientBuilder::default().layer(retry_layer).transport(transport, is_local);

        if !is_local {
            client.set_poll_interval(
                chain
                    .average_blocktime_hint()
                    // we cap the poll interval because if not provided, chain would default to
                    // mainnet
                    .map(|hint| hint.min(DEFAULT_UNKNOWN_CHAIN_BLOCK_TIME))
                    .unwrap_or(DEFAULT_UNKNOWN_CHAIN_BLOCK_TIME)
                    .mul_f32(POLL_INTERVAL_BLOCK_TIME_SCALE_FACTOR),
            );
        }

        let provider = AlloyProviderBuilder::<_, _, N>::default()
            .with_recommended_fillers()
            .wallet(wallet)
            .connect_provider(RootProvider::new(client));

        Ok(provider)
    }
}

#[cfg(not(windows))]
fn resolve_path(path: &Path) -> Result<PathBuf, ()> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir().map(|d| d.join(path)).map_err(drop)
    }
}

#[cfg(windows)]
fn resolve_path(path: &Path) -> Result<PathBuf, ()> {
    if let Some(s) = path.to_str()
        && s.starts_with(r"\\.\pipe\")
    {
        return Ok(path.to_path_buf());
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir().map(|d| d.join(path)).map_err(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_auto_correct_missing_prefix() {
        let builder = ProviderBuilder::<AnyNetwork>::new("localhost:8545");
        assert!(builder.url.is_ok());

        let url = builder.url.unwrap();
        assert_eq!(url, Url::parse("http://localhost:8545").unwrap());
    }

    #[test]
    fn from_config_applies_rpc_transport_options() {
        let config = Config {
            eth_rpc_url: Some("http://example.com".to_string()),
            eth_rpc_accept_invalid_certs: true,
            eth_rpc_no_proxy: true,
            eth_rpc_timeout: Some(7),
            ..Default::default()
        };

        let builder = ProviderBuilder::<AnyNetwork>::from_config(&config).unwrap();

        assert!(builder.accept_invalid_certs);
        assert!(builder.no_proxy);
        assert_eq!(builder.timeout, Duration::from_secs(7));
    }

    // --- Tron nonce shim -------------------------------------------------------------------------

    fn single_request(method: &str) -> RequestPacket {
        RequestPacket::Single(
            alloy_json_rpc::Request::new(method.to_string(), Id::Number(1), ())
                .serialize()
                .unwrap(),
        )
    }

    /// The shim answers `eth_getTransactionCount` with `0x0` without ever touching the inner
    /// transport (java-tron returns a permanent `-32601` for it).
    #[tokio::test]
    async fn tron_shim_intercepts_get_transaction_count() {
        let inner_calls = Arc::new(AtomicUsize::new(0));
        let calls = inner_calls.clone();
        let inner = tower::service_fn(move |_req: RequestPacket| -> TransportFut<'static> {
            calls.fetch_add(1, Ordering::SeqCst);
            // Mimic java-tron's -32601 stub so a wrongful forward fails the test loudly.
            Box::pin(async move {
                Err(alloy_transport::TransportErrorKind::custom_str(
                    "inner transport must not be called for eth_getTransactionCount",
                ))
            })
        });

        let mut service = TronNonceShimLayer.layer(inner);
        let res = service.call(single_request(ETH_GET_TRANSACTION_COUNT)).await.unwrap();

        // The inner transport was never invoked.
        assert_eq!(inner_calls.load(Ordering::SeqCst), 0);

        let ResponsePacket::Single(response) = res else { panic!("expected a single response") };
        let ResponsePayload::Success(raw) = response.payload else {
            panic!("expected a success payload")
        };
        assert_eq!(raw.get(), "\"0x0\"");
    }

    /// Every other method passes straight through to the inner transport untouched.
    #[tokio::test]
    async fn tron_shim_passes_other_methods_through() {
        let inner_calls = Arc::new(AtomicUsize::new(0));
        let calls = inner_calls.clone();
        let inner = tower::service_fn(move |req: RequestPacket| -> TransportFut<'static> {
            calls.fetch_add(1, Ordering::SeqCst);
            let id = req.as_single().unwrap().id().clone();
            Box::pin(async move {
                let raw = to_raw_value(&"0xdeadbeef").unwrap();
                Ok(ResponsePacket::Single(Response { id, payload: ResponsePayload::Success(raw) }))
            })
        });

        let mut service = TronNonceShimLayer.layer(inner);
        let res = service.call(single_request("eth_blockNumber")).await.unwrap();

        // The inner transport handled the request.
        assert_eq!(inner_calls.load(Ordering::SeqCst), 1);

        let ResponsePacket::Single(response) = res else { panic!("expected a single response") };
        let ResponsePayload::Success(raw) = response.payload else {
            panic!("expected a success payload")
        };
        assert_eq!(raw.get(), "\"0xdeadbeef\"");
    }
}
