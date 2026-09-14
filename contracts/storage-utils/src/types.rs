// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Error {
    KeyNotFound = 1,
    IndexOutOfBounds = 2,
    QueueEmpty = 3,
    StackEmpty = 4,
    Overflow = 5,
}
