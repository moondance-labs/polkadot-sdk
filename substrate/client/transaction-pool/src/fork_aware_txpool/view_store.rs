// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Transaction pool view store. Basically block hash to view map with some utitlity methods.

use crate::graph;
use futures::prelude::*;
use parking_lot::RwLock;
use std::{collections::HashMap, sync::Arc};

use crate::graph::ExtrinsicHash;
use sc_transaction_pool_api::{PoolStatus, TransactionSource};

use super::multi_view_listener::{MultiViewListener, TxStatusStream};
use crate::{ReadyIteratorFor, LOG_TARGET};
use sp_blockchain::TreeRoute;
use sp_runtime::{generic::BlockId, traits::Block as BlockT};

use super::view::View;

/// The helper structure encapsulates all the views.
pub(super) struct ViewStore<ChainApi, Block>
where
	Block: BlockT,
	ChainApi: graph::ChainApi<Block = Block>,
{
	/// The blockchain api.
	pub(super) api: Arc<ChainApi>,
	/// Views at tips of the forks.
	pub(super) views: RwLock<HashMap<Block::Hash, Arc<View<ChainApi>>>>,
	/// Views at intermediary blocks that are no longer tip of the fork.
	pub(super) retracted_views: RwLock<HashMap<Block::Hash, Arc<View<ChainApi>>>>,
	/// Listener for controlling external watchers of transactions.
	pub(super) listener: Arc<MultiViewListener<ChainApi>>,

	/// Most recent block processed by tx-pool. Used on in API functions that were not changed to
	/// add at parameter.
	pub(super) most_recent_view: RwLock<Option<Block::Hash>>,
}

impl<ChainApi, Block> ViewStore<ChainApi, Block>
where
	Block: BlockT,
	ChainApi: graph::ChainApi<Block = Block> + 'static,
	<Block as BlockT>::Hash: Unpin,
{
	pub(super) fn new(api: Arc<ChainApi>, listener: Arc<MultiViewListener<ChainApi>>) -> Self {
		Self {
			api,
			views: Default::default(),
			retracted_views: Default::default(),
			listener,
			most_recent_view: RwLock::from(None),
		}
	}

	/// Imports a bunch of unverified extrinsics to every view
	pub(super) async fn submit_at(
		&self,
		source: TransactionSource,
		xts: impl IntoIterator<Item = Block::Extrinsic> + Clone,
	) -> HashMap<Block::Hash, Vec<Result<ExtrinsicHash<ChainApi>, ChainApi::Error>>> {
		let results = {
			let views = self.views.read();
			let futs = views
				.iter()
				.map(|(_, view)| {
					let view = view.clone();
					//todo: remove this clone (Arc?)
					let xts = xts.clone();
					async move {
						let r = (view.at.hash, view.submit_many(source, xts.clone()).await);
						r
					}
				})
				.collect::<Vec<_>>();
			futs
		};
		let results = futures::future::join_all(results).await;

		HashMap::<_, _>::from_iter(results.into_iter())
	}

	/// Imports one unverified extrinsic to every view
	pub(super) async fn submit_one(
		&self,
		source: TransactionSource,
		xt: Block::Extrinsic,
	) -> HashMap<Block::Hash, Result<ExtrinsicHash<ChainApi>, ChainApi::Error>> {
		let mut output = HashMap::new();
		let mut result = self.submit_at(source, std::iter::once(xt)).await;
		result.iter_mut().for_each(|(hash, result)| {
			output.insert(
				*hash,
				result
					.pop()
					.expect("for one transaction there shall be exactly one result. qed"),
			);
		});
		output
	}

	/// Import a single extrinsic and starts to watch its progress in the pool.
	pub(super) async fn submit_and_watch(
		&self,
		_at: Block::Hash,
		source: TransactionSource,
		xt: Block::Extrinsic,
	) -> Result<TxStatusStream<ChainApi>, ChainApi::Error> {
		let tx_hash = self.api.hash_and_length(&xt).0;
		let external_watcher = self.listener.create_external_watcher_for_tx(tx_hash).await;
		let results = {
			let views = self.views.read();
			let futs = views
				.iter()
				.map(|(_, view)| {
					let view = view.clone();
					let xt = xt.clone();

					async move {
						let result = view.submit_and_watch(source, xt).await;
						if let Ok(watcher) = result {
							self.listener
								.add_view_watcher_for_tx(
									tx_hash,
									view.at.hash,
									watcher.into_stream().boxed(),
								)
								.await;
							Ok(())
						} else {
							Err(result.unwrap_err())
						}
					}
				})
				.collect::<Vec<_>>();
			futs
		};
		let maybe_watchers = futures::future::join_all(results).await;
		//todo: maybe try_fold + ControlFlow ?
		let maybe_error = maybe_watchers.into_iter().reduce(|mut r, v| {
			if r.is_err() && v.is_ok() {
				r = v;
			}
			r
		});
		if let Some(Err(err)) = maybe_error {
			log::debug!(target: LOG_TARGET, "[{:?}] submit_and_watch: err: {}", tx_hash, err);
			return Err(err);
		};

		Ok(external_watcher.unwrap())
	}

	pub(super) fn status(&self) -> HashMap<Block::Hash, PoolStatus> {
		self.views.read().iter().map(|(h, v)| (*h, v.status())).collect()
	}

	pub(super) fn is_empty(&self) -> bool {
		self.views.read().is_empty()
	}

	/// Finds the best existing view to clone from along the path.
	/// Allows to include all the transactions from the imported blocks (that are on the retracted
	/// path) without additional validation. Tip of retracted fork is usually most recent block
	/// processed by txpool.
	///
	/// ```text
	/// Tree route from R1 to E2.
	///   <- R3 <- R2 <- R1
	///  /
	/// C
	///  \-> E1 -> E2
	/// ```
	/// ```text
	/// Search path is:
	/// [E1, C, R3, R2, R1]
	/// ```
	pub(super) fn find_best_view(
		&self,
		tree_route: &TreeRoute<Block>,
	) -> Option<Arc<View<ChainApi>>> {
		let views = self.views.read();
		let best_view = {
			tree_route
				.retracted()
				.iter()
				.chain(std::iter::once(tree_route.common_block()))
				.chain(tree_route.enacted().iter())
				.rev()
				.find(|block| views.contains_key(&block.hash))
		};
		best_view.map(|h| {
			views.get(&h.hash).expect("hash was just found in the map's keys. qed").clone()
		})
	}

	pub(super) fn ready(&self, at: Block::Hash) -> Option<ReadyIteratorFor<ChainApi>> {
		let maybe_ready = self.views.read().get(&at).map(|v| v.pool.validated_pool().ready());
		let Some(ready) = maybe_ready else { return None };
		Some(Box::new(ready))
	}

	pub(super) fn futures(
		&self,
		at: Block::Hash,
	) -> Option<Vec<graph::base_pool::Transaction<ExtrinsicHash<ChainApi>, Block::Extrinsic>>> {
		self.views
			.read()
			.get(&at)
			.map(|v| v.pool.validated_pool().pool.read().futures().cloned().collect())
	}

	pub(super) async fn finalize_route(
		&self,
		finalized_hash: Block::Hash,
		tree_route: &[Block::Hash],
	) -> Vec<ExtrinsicHash<ChainApi>> {
		log::debug!(target: LOG_TARGET, "finalize_route finalized_hash:{finalized_hash:?} tree_route: {tree_route:?}");

		let mut finalized_transactions = Vec::new();

		for block in tree_route.iter().chain(std::iter::once(&finalized_hash)) {
			let extrinsics = self
				.api
				.block_body(*block)
				.await
				.unwrap_or_else(|e| {
					log::warn!(target: LOG_TARGET, "Finalize route: error request: {}", e);
					None
				})
				.unwrap_or_default()
				.iter()
				.map(|e| self.api.hash_and_length(e).0)
				.collect::<Vec<_>>();

			let futs = extrinsics
				.iter()
				.enumerate()
				.map(|(i, tx_hash)| self.listener.finalize_transaction(*tx_hash, *block, i))
				.collect::<Vec<_>>();

			finalized_transactions.extend(extrinsics);
			future::join_all(futs).await;
		}

		finalized_transactions
	}

	pub(super) fn ready_transaction(
		&self,
		at: Block::Hash,
		tx_hash: &ExtrinsicHash<ChainApi>,
	) -> Option<Arc<graph::base_pool::Transaction<ExtrinsicHash<ChainApi>, Block::Extrinsic>>> {
		self.views
			.read()
			.get(&at)
			.and_then(|v| v.pool.validated_pool().ready_by_hash(tx_hash))
	}

	pub(super) async fn insert_new_view(
		&self,
		view: Arc<View<ChainApi>>,
		tree_route: &TreeRoute<Block>,
	) {
		let mut views_to_be_removed = {
			std::iter::once(tree_route.common_block())
				.chain(tree_route.enacted().iter())
				.map(|block| block.hash)
				.collect::<Vec<_>>()
		};
		//todo: refactor this: maybe single object with one mutex?
		let views_to_be_removed = {
			let mut most_recent_view_lock = self.most_recent_view.write();
			let mut views = self.views.write();
			let mut retracted_views = self.retracted_views.write();
			views_to_be_removed.retain(|hash| {
				let view = views.remove(hash);
				if let Some(view) = view {
					retracted_views.insert(*hash, view);
					true
				} else {
					false
				}
			});
			views.insert(view.at.hash, view.clone());
			most_recent_view_lock.replace(view.at.hash);
			views_to_be_removed
		};
		{
			log::debug!(target:LOG_TARGET,"insert_new_view: retracted_views: {:?}", self.retracted_views.read().keys());
		}
		for hash in &views_to_be_removed {
			self.listener.remove_view(*hash).await;
		}
	}

	pub(super) fn get_view_at(
		&self,
		at: Block::Hash,
		allow_retracted: bool,
	) -> Option<(Arc<View<ChainApi>>, bool)> {
		if let Some(view) = self.views.read().get(&at) {
			return Some((view.clone(), false));
		}
		if allow_retracted {
			if let Some(view) = self.retracted_views.read().get(&at) {
				return Some((view.clone(), true))
			}
		};
		None
	}

	pub(crate) async fn handle_finalized(
		&self,
		finalized_hash: Block::Hash,
		tree_route: &[Block::Hash],
	) -> Vec<ExtrinsicHash<ChainApi>> {
		let finalized_xts = self.finalize_route(finalized_hash, tree_route).await;

		let finalized_number = self.api.block_id_to_number(&BlockId::Hash(finalized_hash));

		//clean up older then finalized
		{
			let mut views = self.views.write();
			views.retain(|hash, v| match finalized_number {
				Err(_) | Ok(None) => *hash == finalized_hash,
				Ok(Some(n)) if v.at.number == n => *hash == finalized_hash,
				Ok(Some(n)) => v.at.number > n,
			});
		}

		{
			let mut retracted_views = self.retracted_views.write();
			retracted_views.retain(|_, v| match finalized_number {
				Err(_) | Ok(None) => false,
				Ok(Some(n)) => v.at.number > n,
			});

			log::debug!(target:LOG_TARGET,"handle_finalized: retracted_views: {:?}", retracted_views.keys());
		}

		finalized_xts
	}
}
