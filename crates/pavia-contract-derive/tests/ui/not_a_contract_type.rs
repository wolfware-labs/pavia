use std::time::Duration;

use pavia_contract::Contract;

#[derive(Contract)]
struct Timeout {
  after: Duration,
}

fn main() {}
