use crate::tx::{CastTxBuilder, SenderKind};
use alloy_ens::NameOrAddress;
use alloy_network::{Ethereum, Network};
use alloy_primitives::{Address, U256};
use alloy_provider::Provider;
use alloy_rpc_types::BlockId;
use clap::Parser;
use eyre::Result;
use foundry_cli::{
    json::print_scalar,
    opts::{RpcOpts, TransactionOpts},
    utils::{LoadConfig, parse_ether_value},
};
use foundry_common::{FoundryTransactionBuilder, provider::ProviderBuilder};
use foundry_config::Config;
use foundry_tron_provider::{
    DEFAULT_FEE_LIMIT_BUFFER_PCT, TronChainParams, estimate_call_bandwidth, suggest_fee_limit_sun,
};
use foundry_wallets::WalletOpts;
use tempo_alloy::TempoNetwork;

/// CLI arguments for `cast estimate`.
#[derive(Debug, Parser)]
pub struct EstimateArgs {
    /// The destination of the transaction.
    #[arg(value_parser = crate::tron::parse_name_or_tron_address)]
    to: Option<NameOrAddress>,

    /// The signature of the function to call.
    sig: Option<String>,

    /// The arguments of the function to call.
    #[arg(allow_negative_numbers = true)]
    args: Vec<String>,

    /// The block height to query at.
    ///
    /// Can also be the tags earliest, finalized, safe, latest, or pending.
    #[arg(long, short = 'B')]
    block: Option<BlockId>,

    /// Calculate the cost of a transaction using the network gas price.
    ///
    /// If not specified the amount of gas will be estimated.
    #[arg(long)]
    cost: bool,

    #[command(flatten)]
    wallet: WalletOpts,

    #[command(subcommand)]
    command: Option<EstimateSubcommands>,

    #[command(flatten)]
    tx: TransactionOpts,

    #[command(flatten)]
    rpc: RpcOpts,
}

#[derive(Debug, Parser)]
pub enum EstimateSubcommands {
    /// Estimate gas cost to deploy a smart contract
    #[command(name = "--create")]
    Create {
        /// The bytecode of contract
        code: String,

        /// The signature of the constructor
        sig: Option<String>,

        /// Constructor arguments
        #[arg(allow_negative_numbers = true)]
        args: Vec<String>,

        /// Ether to send in the transaction
        ///
        /// Either specified in wei, or as a string with a unit type:
        ///
        /// Examples: 1ether, 10gwei, 0.01ether
        #[arg(long, value_parser = parse_ether_value)]
        value: Option<U256>,
    },
}

impl EstimateArgs {
    pub async fn run(self) -> Result<()> {
        // Tron is config-file driven (`network = "tron"`); it diverges before any alloy
        // network dispatch because energy/bandwidth are estimated over the `/wallet/*`
        // HTTP API, not `eth_estimateGas`.
        let config = self.rpc.load_config()?;
        if config.networks.is_tron() {
            return self.run_tron(config).await;
        }
        if self.tx.tempo.is_tempo() {
            self.run_with_network::<TempoNetwork>().await
        } else {
            self.run_with_network::<Ethereum>().await
        }
    }

    /// Estimates a Tron contract call: the energy (TIP-491 penalty included), the
    /// on-chain bandwidth, a suggested `fee_limit` and the burned-cost estimate.
    ///
    /// Energy comes from `/wallet/estimateenergy` when the node supports it, else
    /// from a constant call whose `energy_used` already carries the dynamic-energy
    /// penalty. Pricing and the fee-limit ceiling come from the node's live chain
    /// parameters, falling back to the 2026-07 snapshot (with a warning) when the
    /// node cannot be reached. The estimate is the command's result and prints to
    /// stdout (a table, or an object under `--json`).
    async fn run_tron(self, config: Config) -> Result<()> {
        if self.command.is_some() {
            eyre::bail!(
                "cast estimate --create is not supported on tron yet; use `forge create` to deploy"
            );
        }
        let to = self.to.ok_or_else(|| {
            eyre::eyre!("a destination contract address is required for a tron estimate")
        })?;
        let contract = crate::tron::parse_tron_address(&crate::tron::name_or_address_str(&to))?;
        let owner = self.wallet.from.unwrap_or(Address::ZERO);

        let sig = self.sig.as_deref();
        let (data, _func) = crate::tron::encode_calldata(sig, &self.args)?;

        let provider = crate::tron::tron_provider(&config)?;

        // Energy: prefer `estimateenergy`; on a node that disables it, fall back to a
        // constant call whose `energy_used` already includes the TIP-491 penalty.
        let (energy_used, energy_penalty) =
            match provider.estimate_energy(owner, contract, &data).await? {
                Some(required) => (required, 0),
                None => {
                    let cr = provider.trigger_constant(owner, contract, &data).await?;
                    if !cr.success {
                        eyre::bail!("tron energy estimate reverted");
                    }
                    (cr.energy_used, cr.energy_penalty)
                }
            };

        // Live chain parameters for pricing and the fee-limit suggestion; fall back
        // to the 2026-07 snapshot (and warn) on a network error.
        let params = match provider.get_chain_parameters().await {
            Ok(p) => p,
            Err(e) => {
                sh_warn!(
                    "could not fetch tron chain parameters ({e}); using the 2026-07 defaults"
                )?;
                TronChainParams::default()
            }
        };

        let opts = crate::tron::tx_options(&config.tron);
        let value_sun = crate::tron::value_sun(self.tx.value)?;
        let bandwidth_bytes = estimate_call_bandwidth(data, value_sun, &opts);
        let suggested_fee_limit_sun =
            suggest_fee_limit_sun(energy_used, &params, DEFAULT_FEE_LIMIT_BUFFER_PCT);
        let est_cost_sun = ((energy_used as u128) * (params.energy_fee_sun as u128)
            + (bandwidth_bytes as u128) * (params.transaction_fee_sun as u128))
            .min(u64::MAX as u128) as u64;

        crate::tron::print_tron_estimate(&crate::tron::TronEstimate {
            energy_used,
            energy_penalty,
            bandwidth_bytes,
            suggested_fee_limit_sun,
            est_cost_sun,
        })
    }

    pub async fn run_with_network<N: Network>(self) -> Result<()>
    where
        N::TransactionRequest: FoundryTransactionBuilder<N>,
    {
        let Self { to, mut sig, mut args, mut tx, block, cost, wallet, rpc, command } = self;

        let config = rpc.load_config()?;
        let provider = ProviderBuilder::<N>::from_config(&config)?.build()?;
        let sender = SenderKind::from_wallet_opts(wallet).await?;

        let code = if let Some(EstimateSubcommands::Create {
            code,
            sig: create_sig,
            args: create_args,
            value,
        }) = command
        {
            sig = create_sig;
            args = create_args;
            if let Some(value) = value {
                tx.value = Some(value);
            }
            Some(code)
        } else {
            None
        };

        let (tx, _) = CastTxBuilder::new(&provider, tx, &config)
            .await?
            .with_to(to)
            .await?
            .with_code_sig_and_args(code, sig, args)
            .await?
            .raw()
            .build(sender)
            .await?;

        let gas = provider.estimate_gas(tx).block(block.unwrap_or_default()).await?;
        if cost {
            let gas_price_wei = provider.get_gas_price().await?;
            let cost = gas_price_wei * gas as u128;
            let cost_eth = cost as f64 / 1e18;
            print_scalar(cost_eth)?;
        } else {
            print_scalar(gas)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_estimate_value() {
        let args: EstimateArgs = EstimateArgs::parse_from(["foundry-cli", "--value", "100"]);
        assert!(args.tx.value.is_some());
    }
}
