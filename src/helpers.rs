use crate::{chain, opt::Solver, prelude::*, static_types};
use frame_election_provider_support::{PhragMMS, SequentialPhragmen};
use frame_support::{weights::Weight, BoundedVec};
use pallet_election_provider_multi_phase::{SolutionOf, SolutionOrSnapshotSize};
use pin_project_lite::pin_project;
use sp_npos_elections::ElectionScore;
use std::{
	future::Future,
	pin::Pin,
	task::{Context, Poll},
	time::{Duration, Instant},
};

pin_project! {
	pub struct Timed<Fut>
		where
		Fut: Future,
	{
		#[pin]
		inner: Fut,
		start: Option<Instant>,
	}
}

impl<Fut> Future for Timed<Fut>
where
	Fut: Future,
{
	type Output = (Fut::Output, Duration);

	fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
		let this = self.project();
		let start = this.start.get_or_insert_with(Instant::now);

		match this.inner.poll(cx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(v) => {
				let elapsed = start.elapsed();
				Poll::Ready((v, elapsed))
			},
		}
	}
}

pub trait TimedFuture: Sized + Future {
	fn timed(self) -> Timed<Self> {
		Timed { inner: self, start: None }
	}
}

impl<F: Future> TimedFuture for F {}

macro_rules! helpers_for_runtime {
	($runtime:tt) => {
		paste::paste! {
			/// The monitor command.
			pub(crate) async fn [<mine_solution_$runtime>](
				api: &SubxtClient,
				hash: Option<Hash>,
				solver: Solver
			) -> Result<(SolutionOf<chain::$runtime::Config>, ElectionScore, SolutionOrSnapshotSize), Error> {

				let (voters, targets, desired_targets) = [<snapshot_$runtime>](&api, hash).await?;

				let blocking_task = tokio::task::spawn_blocking(move || {
					match solver {
						Solver::SeqPhragmen { iterations } => {
							BalanceIterations::set(iterations);
							Miner::<chain::$runtime::Config>::mine_solution_with_snapshot::<
								SequentialPhragmen<AccountId, Accuracy, Balancing>,
							>(voters, targets, desired_targets)
						},
						Solver::PhragMMS { iterations } => {
							BalanceIterations::set(iterations);
							Miner::<chain::$runtime::Config>::mine_solution_with_snapshot::<PhragMMS<AccountId, Accuracy, Balancing>>(
								voters,
								targets,
								desired_targets,
							)
						},
					}
				}).await;

				match blocking_task {
					Ok(Ok(res)) => Ok(res),
					Ok(Err(err)) => Err(Error::Other(format!("{:?}", err))),
					Err(err) => Err(Error::Other(format!("{:?}", err))),
				}
			}

			pub async fn [<snapshot_$runtime>](api: &SubxtClient, hash: Option<Hash>) -> Result<crate::chain::$runtime::epm::Snapshot, Error> {
				use crate::chain::[<$runtime>]::{epm::RoundSnapshot, runtime};
				use crate::static_types;

				let RoundSnapshot { voters, targets } = api
					.storage().fetch(&runtime::storage().election_provider_multi_phase().snapshot(), hash)
					.await?
					.unwrap_or_default();

				let desired_targets = api
					.storage()
					.fetch(&runtime::storage().election_provider_multi_phase().desired_targets(), hash)
					.await?
					.unwrap_or_default();

				let voters: Vec<_> = voters
					.into_iter()
					.map(|(a, b, mut c)| {
						let mut bounded_vec: BoundedVec<AccountId, static_types::MaxVotesPerVoter> = BoundedVec::default();
						// If this fails just crash the task.
						bounded_vec.try_append(&mut c.0).unwrap_or_else(|_| panic!("BoundedVec capacity: {} failed; `MinerConfig::MaxVotesPerVoter` is different from the chain data; this is a bug please file an issue", static_types::MaxVotesPerVoter::get()));
						(a, b, bounded_vec)
					})
					.collect();

				Ok((voters, targets, desired_targets))
			}
		}
	};
}

#[cfg(feature = "polkadot")]
helpers_for_runtime!(polkadot);
#[cfg(feature = "kusama")]
helpers_for_runtime!(kusama);
#[cfg(feature = "westend")]
helpers_for_runtime!(westend);

pub(crate) async fn read_metadata_constants(api: &SubxtClient) -> Result<(), Error> {
	let max_weight = {
		let val = api
			.constants()
			.at(&subxt::dynamic::constant("ElectionProviderMultiPhase", "SignedMaxWeight"))?;

		deserialize_scale_value::<Weight>(val)?
	};

	let max_length: u32 = {
		let val = api
			.constants()
			.at(&subxt::dynamic::constant("ElectionProviderMultiPhase", "MinerMaxLength"))
			.expect("MinerMaxLength");

		deserialize_scale_value::<u32>(val)?
	};

	let max_votes_per_voter: u32 = {
		let val = api
			.constants()
			.at(&subxt::dynamic::constant("ElectionProviderMultiPhase", "MinerMaxVotesPerVoter"))
			.expect("MinerMaxVotesPerVoter");

		deserialize_scale_value::<u32>(val)?
	};

	static_types::MaxWeight::set(max_weight);
	static_types::MaxLength::set(max_length);
	static_types::MaxVotesPerVoter::set(max_votes_per_voter);

	Ok(())
}

fn deserialize_scale_value<'a, T: serde::Deserialize<'a>>(
	val: scale_value::Value<scale_value::scale::TypeId>,
) -> Result<T, Error> {
	scale_value::serde::from_value::<_, T>(val).map_err(|e| Error::Other(e.to_string()))
}
