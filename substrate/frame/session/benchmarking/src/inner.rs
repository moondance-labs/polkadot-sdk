// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Benchmarks for the Session Pallet.
// This is separated into its own crate due to cyclic dependency issues.

use alloc::{vec, vec::Vec};
use core::marker::PhantomData;
use sp_runtime::traits::{One, StaticLookup, TrailingZeroInput};

use codec::Decode;
use frame_benchmarking::v2::*;
use frame_support::traits::{Get, KeyOwnerProofSystem, OnInitialize};
use frame_system::{pallet_prelude::BlockNumberFor, RawOrigin};
use pallet_session::{historical::Pallet as Historical, Pallet as Session, *};
use pallet_staking::{
	benchmarking::create_validator_with_nominators, testing_utils::create_validators,
	MaxNominationsOf, RewardDestination,
};

const MAX_VALIDATORS: u32 = 1000;

pub struct Pallet<T: Config>(pallet_session::Pallet<T>);
pub trait Config: pallet_session::Config + pallet_session::historical::Config {
	type StakingAdapter: StakingAdapter<Self::AccountId>;
}

impl<T: Config> OnInitialize<BlockNumberFor<T>> for Pallet<T> {
	fn on_initialize(n: BlockNumberFor<T>) -> frame_support::weights::Weight {
		pallet_session::Pallet::<T>::on_initialize(n)
	}
}

/// Adapter to wire benchmarks to various staking systems.
pub trait StakingAdapter<AccountId> {
	fn max_nominations() -> u32;

	fn set_validators_count(count: u32);

	fn controller_for_stash_account(a: &AccountId) -> Option<AccountId>;

	fn create_validator_with_nominators(nominations: u32, max_nominations: u32) -> AccountId;

	fn create_many_validators(n: u32) -> Vec<AccountId>;
}

pub struct PalletStaking<T>(PhantomData<T>);
impl<T: pallet_staking::Config> StakingAdapter<T::AccountId> for PalletStaking<T> {
	fn max_nominations() -> u32 {
		MaxNominationsOf::<T>::get()
	}

	fn set_validators_count(count: u32) {
		pallet_staking::ValidatorCount::<T>::put(count);
	}

	fn controller_for_stash_account(a: &T::AccountId) -> Option<T::AccountId> {
		pallet_staking::Pallet::<T>::bonded(&a)
	}

	fn create_validator_with_nominators(nominations: u32, max_nominations: u32) -> T::AccountId {
		let (stash, _) = create_validator_with_nominators::<T>(
			nominations,
			max_nominations,
			false,
			true,
			RewardDestination::Staked,
		)
		.expect("failed to create validator");

		stash
	}

	fn create_many_validators(n: u32) -> Vec<T::AccountId> {
		create_validators::<T>(n, 1000)
			.expect("failed to create validators")
			.into_iter()
			.map(|who| T::Lookup::lookup(who).unwrap())
			.collect()
	}
}

#[benchmarks]
mod benchmarks {
	use super::*;

	#[benchmark]
	fn set_keys() -> Result<(), BenchmarkError> {
		let n = T::StakingAdapter::max_nominations();
		let v_stash = T::StakingAdapter::create_validator_with_nominators(
			n,
			T::StakingAdapter::max_nominations(),
		);
		let v_controller =
			T::StakingAdapter::controller_for_stash_account(&v_stash).ok_or("not stash")?;

		let keys = T::Keys::decode(&mut TrailingZeroInput::zeroes()).unwrap();
		let proof: Vec<u8> = vec![0, 1, 2, 3];
		// Whitelist controller account from further DB operations.
		let v_controller_key = frame_system::Account::<T>::hashed_key_for(&v_controller);
		frame_benchmarking::benchmarking::add_to_whitelist(v_controller_key.into());

		#[extrinsic_call]
		_(RawOrigin::Signed(v_controller), keys, proof);

		Ok(())
	}

	#[benchmark]
	fn purge_keys() -> Result<(), BenchmarkError> {
		let n = T::StakingAdapter::max_nominations();
		let v_stash = T::StakingAdapter::create_validator_with_nominators(
			n,
			T::StakingAdapter::max_nominations(),
		);
		let v_controller =
			T::StakingAdapter::controller_for_stash_account(&v_stash).ok_or("not stash")?;
		let keys = T::Keys::decode(&mut TrailingZeroInput::zeroes()).unwrap();
		let proof: Vec<u8> = vec![0, 1, 2, 3];
		Session::<T>::set_keys(RawOrigin::Signed(v_controller.clone()).into(), keys, proof)?;
		// Whitelist controller account from further DB operations.
		let v_controller_key = frame_system::Account::<T>::hashed_key_for(&v_controller);
		frame_benchmarking::benchmarking::add_to_whitelist(v_controller_key.into());

		#[extrinsic_call]
		_(RawOrigin::Signed(v_controller));

		Ok(())
	}

	#[benchmark(extra)]
	fn check_membership_proof_current_session(n: Linear<2, MAX_VALIDATORS>) {
		let (key, key_owner_proof1) = check_membership_proof_setup::<T>(n);
		let key_owner_proof2 = key_owner_proof1.clone();

		#[block]
		{
			Historical::<T>::check_proof(key, key_owner_proof1);
		}

		assert!(Historical::<T>::check_proof(key, key_owner_proof2).is_some());
	}

	#[benchmark(extra)]
	fn check_membership_proof_historical_session(n: Linear<2, MAX_VALIDATORS>) {
		let (key, key_owner_proof1) = check_membership_proof_setup::<T>(n);

		// skip to the next session so that the session is historical
		// and the membership merkle proof must be checked.
		Session::<T>::rotate_session();

		let key_owner_proof2 = key_owner_proof1.clone();

		#[block]
		{
			Historical::<T>::check_proof(key, key_owner_proof1);
		}

		assert!(Historical::<T>::check_proof(key, key_owner_proof2).is_some());
	}

	impl_benchmark_test_suite!(
		Pallet,
		crate::mock::new_test_ext(),
		crate::mock::Test,
		extra = false
	);
}

/// Sets up the benchmark for checking a membership proof. It creates the given
/// number of validators, sets random session keys and then creates a membership
/// proof for the first authority and returns its key and the proof.
fn check_membership_proof_setup<T: Config>(
	n: u32,
) -> ((sp_runtime::KeyTypeId, &'static [u8; 32]), sp_session::MembershipProof) {
	T::StakingAdapter::set_validators_count(n);

	// create validators and set random session keys
	for (n, validator) in T::StakingAdapter::create_many_validators(n).into_iter().enumerate() {
		use rand::{RngCore, SeedableRng};

		let controller = T::StakingAdapter::controller_for_stash_account(&validator).unwrap();

		let keys = {
			let mut keys = [0u8; 128];

			// we keep the keys for the first validator as 0x00000...
			if n > 0 {
				let mut rng = rand::rngs::StdRng::seed_from_u64(n as u64);
				rng.fill_bytes(&mut keys);
			}

			keys
		};

		let keys: T::Keys = Decode::decode(&mut &keys[..]).unwrap();
		let proof: Vec<u8> = vec![];

		Session::<T>::set_keys(RawOrigin::Signed(controller).into(), keys, proof).unwrap();
	}

	Pallet::<T>::on_initialize(frame_system::pallet_prelude::BlockNumberFor::<T>::one());

	// skip sessions until the new validator set is enacted
	while Validators::<T>::get().len() < n as usize {
		Session::<T>::rotate_session();
	}

	let key = (sp_runtime::KeyTypeId(*b"babe"), &[0u8; 32]);

	(key, Historical::<T>::prove(key).unwrap())
}
